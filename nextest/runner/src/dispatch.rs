// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    partition::PartitionerBuilder,
    reporter::{Color, TestReporter},
    runner::TestRunnerOpts,
    signal::SignalHandler,
    test_filter::{RunIgnored, TestFilterBuilder},
    test_list::{OutputFormat, TestBinary, TestList},
};
use camino::{Utf8Path, Utf8PathBuf};
use color_eyre::eyre::{bail, Result, WrapErr};
use duct::cmd;
use nextest_config::{errors::ConfigReadError, NextestConfig};
use structopt::StructOpt;
use supports_color::Stream;
use crate::reporter::ReporterOpts;

/// This test runner accepts a Rust test binary and does fancy things with it.
///
/// TODO: expand on this
#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab-case")]
pub struct Opts {
    #[structopt(long, default_value)]
    /// Coloring: always, auto, never
    color: Color,

    #[structopt(flatten)]
    config_opts: ConfigOpts,

    #[structopt(subcommand)]
    command: Command,
}

#[derive(Debug, StructOpt)]
pub struct ConfigOpts {
    /// Config file [default: workspace-root/.config/nextest.toml]
    #[structopt(long)]
    pub config_file: Option<Utf8PathBuf>,
}

impl ConfigOpts {
    /// Creates a nextest config with the given options.
    pub fn make_config(&self, workspace_root: &Utf8Path) -> Result<NextestConfig, ConfigReadError> {
        NextestConfig::from_sources(workspace_root, self.config_file.as_deref())
    }
}

#[derive(Debug, StructOpt)]
pub enum Command {
    /// List tests in binary
    ListTests {
        /// Output format
        #[structopt(short = "T", long, default_value, possible_values = & OutputFormat::variants(), case_insensitive = true)]
        format: OutputFormat,

        #[structopt(flatten)]
        bin_filter: TestBinFilter,
    },
    /// Run tests
    Run {
        /// Nextest profile to use
        #[structopt(long, short = "P")]
        profile: Option<String>,
        #[structopt(flatten)]
        bin_filter: TestBinFilter,
        #[structopt(flatten)]
        runner_opts: TestRunnerOpts,
        #[structopt(flatten)]
        reporter_opts: ReporterOpts,
    },
}

#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab-case")]
pub struct TestBinFilter {
    /// Path to test binary
    #[structopt(
        short = "b",
        long,
        required = true,
        min_values = 1,
        number_of_values = 1
    )]
    pub test_bin: Vec<Utf8PathBuf>,

    /// Run ignored tests
    #[structopt(long, possible_values = &RunIgnored::variants(), default_value, case_insensitive = true)]
    pub run_ignored: RunIgnored,

    /// Test partition, e.g. hash:1/2 or count:2/3
    #[structopt(long)]
    pub partition: Option<PartitionerBuilder>,

    // TODO: add regex-based filtering in the future?
    /// Test filter
    pub filter: Vec<String>,
}

impl TestBinFilter {
    fn compute(&self) -> Result<TestList> {
        let test_filter =
            TestFilterBuilder::new(self.run_ignored, self.partition.clone(), &self.filter);
        TestList::new(
            self.test_bin.iter().map(|binary| TestBinary {
                binary: binary.clone(),
                // TODO: add support for these through the CLI interface?
                binary_id: binary.clone().into(),
                cwd: None,
            }),
            &test_filter,
        )
    }
}

impl Opts {
    /// Execute the command.
    pub fn exec(self) -> Result<()> {
        let stdout = std::io::stdout();

        match self.command {
            Command::ListTests { bin_filter, format } => {
                let mut test_list = bin_filter.compute()?;
                if self.color.should_colorize(Stream::Stdout) {
                    test_list.colorize();
                }
                let lock = stdout.lock();
                test_list.write(format, lock)?;
            }
            Command::Run {
                ref profile,
                ref bin_filter,
                ref runner_opts,
                ref reporter_opts,
            } => {
                let workspace_root = workspace_root()?;
                let config = self.config_opts.make_config(&workspace_root)?;
                let profile =
                    config.profile(profile.as_deref().unwrap_or(NextestConfig::DEFAULT_PROFILE))?;
                let metadata_dir = profile.metadata_dir();
                std::fs::create_dir_all(&metadata_dir).wrap_err_with(|| {
                    format!("failed to create metadata dir '{}'", metadata_dir)
                })?;

                let test_list = bin_filter.compute()?;

                let mut reporter = TestReporter::new(&test_list, &profile, reporter_opts);
                if self.color.should_colorize(Stream::Stdout) {
                    reporter.colorize();
                }

                let handler = SignalHandler::new().wrap_err("failed to set up Ctrl-C handler")?;
                let runner = runner_opts.build(&test_list, &profile, handler);
                let run_stats = runner.try_execute(|event| {
                    // TODO: consider turning this into a trait, to initialize and carry the lock
                    // across callback invocations
                    let lock = stdout.lock();
                    reporter.report_event(event, lock)
                    // TODO: no-fail-fast logic
                })?;
                if !run_stats.is_success() {
                    bail!("test run failed");
                }
            }
        }
        Ok(())
    }
}

// TODO: replace with guppy
fn workspace_root() -> Result<Utf8PathBuf> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    cmd!(
        cargo,
        "locate-project",
        "--workspace",
        "--message-format",
        "plain"
    )
    .read()
    .map(|s| {
        let mut p = Utf8PathBuf::from(s);
        p.pop();
        p
    })
    .wrap_err_with(|| "cargo locate-project failed")
}
