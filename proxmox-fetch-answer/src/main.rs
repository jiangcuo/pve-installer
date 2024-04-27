use std::process::ExitCode;
use std::{fs, path::PathBuf};

use anyhow::{bail, format_err, Result};
use log::{error, info, LevelFilter};

use proxmox_auto_installer::{
    log::AutoInstLogger,
    utils::{AutoInstSettings, FetchAnswerFrom, HttpOptions},
};

use fetch_plugins::{http::FetchFromHTTP, partition::FetchFromPartition};

mod fetch_plugins;

static LOGGER: AutoInstLogger = AutoInstLogger;
static AUTOINST_MODE_FILE: &str = "/cdrom/auto-installer-mode.toml";

pub fn init_log() -> Result<()> {
    AutoInstLogger::init("/tmp/fetch_answer.log")?;
    log::set_logger(&LOGGER)
        .map(|()| log::set_max_level(LevelFilter::Info))
        .map_err(|err| format_err!(err))
}

fn fetch_answer(install_settings: &AutoInstSettings) -> Result<String> {
    info!("Fetching answer file in mode {:?}:", &install_settings.mode);
    match install_settings.mode {
        FetchAnswerFrom::Iso => {
            let answer_path = PathBuf::from("/cdrom/answer.toml");
            match fs::read_to_string(answer_path) {
                Ok(answer) => return Ok(answer),
                Err(err) => info!("Fetching answer file from ISO failed: {err}"),
            }
        }
        FetchAnswerFrom::Partition => match FetchFromPartition::get_answer() {
            Ok(answer) => return Ok(answer),
            Err(err) => info!("Fetching answer file from partition failed: {err}"),
        },
        FetchAnswerFrom::Http => match FetchFromHTTP::get_answer(&install_settings.http) {
            Ok(answer) => return Ok(answer),
            Err(err) => info!("Fetching answer file via HTTP failed: {err}"),
        },
    }
    bail!("Could not find any answer file!");
}

fn settings_from_cli_args(args: &[String]) -> Result<AutoInstSettings> {
    // TODO: this was done in a bit of a hurry, needs tidying up
    let mode = match args[1].to_lowercase().as_str() {
        "iso" => FetchAnswerFrom::Iso,
        "http" => FetchAnswerFrom::Http,
        "partition" => FetchAnswerFrom::Partition,
        "-h" | "--help" => bail!(
            "usage: {} <http|iso|partition> [<http-url>] [<tls-cert-fingerprint>]",
            args[0]
        ),
        _ => bail!("failed to parse fetch-from argument, not one of 'http', 'iso', or 'partition'"),
    };
    if args.len() > 4 {
    } else if args.len() > 2 && mode != FetchAnswerFrom::Http {
        bail!("only 'http' fetch-from mode supports additional url and cert-fingerprint mode");
    }
    Ok(AutoInstSettings {
        mode,
        http: HttpOptions {
            url: args.get(2).cloned(),
            cert_fingerprint: args.get(3).cloned(),
        },
    })
}

fn do_main() -> Result<()> {
    if let Err(err) = init_log() {
        bail!("could not initialize logging: {err}");
    }

    let args: Vec<String> = std::env::args().collect();

    let install_settings: AutoInstSettings = if args.len() > 1 {
        settings_from_cli_args(&args)?
    } else {
        let raw_install_settings = fs::read_to_string(AUTOINST_MODE_FILE).map_err(|err| {
            format_err!(
                "Could not find needed file '{AUTOINST_MODE_FILE}' in live environment: {err}"
            )
        })?;
        toml::from_str(raw_install_settings.as_str())
            .map_err(|err| format_err!("Failed to parse '{AUTOINST_MODE_FILE}': {err}"))?
    };

    let answer = fetch_answer(&install_settings).map_err(|err| format_err!("Aborting: {err}"))?;
    info!("queried answer file for automatic installation successfully");

    println!("{answer}");

    Ok(())
}

fn main() -> ExitCode {
    match do_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            error!("{err}");
            ExitCode::FAILURE
        }
    }
}
