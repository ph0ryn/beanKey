use std::error::Error;
use std::ffi::OsString;
use std::path::PathBuf;

use beankey_daemon::{DaemonConfig, DaemonServer, Engine, ServerError, summarize_metrics};

fn main() {
    if let Err(error) = run() {
        eprintln!("beankey-daemon: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args_os().skip(1).peekable();
    if arguments
        .peek()
        .is_some_and(|argument| argument == "--help")
    {
        arguments.next();
        if arguments.next().is_some() {
            return Err(USAGE.into());
        }
        println!(
            "{USAGE}\n\nMetrics contain counts, modes and timing, not input or candidate text.\nSaved history is never deleted automatically; disk usage grows.\nThe summary reads all saved segments without starting or stopping the daemon.\nAgreement with a chosen candidate is a proxy, not objective accuracy or OS insertion acknowledgement."
        );
        return Ok(());
    }
    if arguments
        .peek()
        .is_some_and(|argument| argument == "--metrics-summary")
    {
        arguments.next();
        let directory = required_absolute_directory(&mut arguments, "--learning-directory")?;
        if arguments.next().is_some() {
            return Err(format!("unexpected summary arguments; {USAGE}").into());
        }
        let summary = summarize_metrics(directory)?;
        use std::io::Write;
        let mut stdout = std::io::stdout().lock();
        serde_json::to_writer_pretty(&mut stdout, &summary)?;
        writeln!(stdout)?;
        return Ok(());
    }
    let arguments = parse_arguments(arguments)?;
    let config = DaemonConfig::load(arguments.config_path)?;
    let reservation = match DaemonServer::reserve(arguments.runtime_root, &config.runtime_socket) {
        Ok(reservation) => reservation,
        Err(ServerError::AlreadyRunning) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let engine = Engine::open_with_config(&config, arguments.learning_directory)?;
    reservation.bind(engine)?.run()?;
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
struct DaemonArguments {
    config_path: PathBuf,
    runtime_root: PathBuf,
    learning_directory: PathBuf,
}

const USAGE: &str = "usage: beankey-daemon --config PATH --runtime-root PATH --learning-directory PATH\n       beankey-daemon --metrics-summary --learning-directory PATH\n       beankey-daemon --help";

fn parse_arguments(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<DaemonArguments, String> {
    let mut arguments = arguments.into_iter();
    let config_path = required_argument(&mut arguments, "--config")?;
    let runtime_root = required_absolute_directory(&mut arguments, "--runtime-root")?;
    let learning_directory = required_absolute_directory(&mut arguments, "--learning-directory")?;
    if arguments.next().is_some() {
        return Err(format!("unexpected daemon arguments; {USAGE}"));
    }
    Ok(DaemonArguments {
        config_path: config_path.into(),
        runtime_root,
        learning_directory,
    })
}

fn required_argument(
    arguments: &mut impl Iterator<Item = OsString>,
    expected_flag: &str,
) -> Result<OsString, String> {
    match arguments.next() {
        Some(flag) if flag == expected_flag => {}
        _ => return Err(USAGE.to_owned()),
    }
    arguments
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{expected_flag} requires a path"))
}

fn required_absolute_directory(
    arguments: &mut impl Iterator<Item = OsString>,
    flag: &str,
) -> Result<PathBuf, String> {
    let path = PathBuf::from(required_argument(arguments, flag)?);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(format!("{flag} must be an absolute path"))
    }
}

#[cfg(test)]
mod tests {
    use super::parse_arguments;
    use std::ffi::OsString;
    use std::path::Path;

    #[test]
    fn parses_explicit_runtime_and_learning_paths() {
        let arguments = parse_arguments(
            [
                "--config",
                "/configuration/config.toml",
                "--runtime-root",
                "/runtime",
                "--learning-directory",
                "/Users/test/Library/Application Support/beanKey/learning",
            ]
            .map(OsString::from),
        )
        .unwrap();

        assert_eq!(
            arguments.config_path,
            Path::new("/configuration/config.toml")
        );
        assert_eq!(arguments.runtime_root, Path::new("/runtime"));
        assert_eq!(
            arguments.learning_directory,
            Path::new("/Users/test/Library/Application Support/beanKey/learning")
        );
    }

    #[test]
    fn rejects_missing_relative_and_unexpected_daemon_arguments() {
        assert!(parse_arguments([OsString::from("--config")]).is_err());
        assert!(
            parse_arguments(
                [
                    "--config",
                    "/configuration/config.toml",
                    "--runtime-root",
                    "relative",
                    "--learning-directory",
                    "/state/learning",
                ]
                .map(OsString::from),
            )
            .is_err()
        );
        assert!(
            parse_arguments(
                [
                    "--config",
                    "/configuration/config.toml",
                    "--runtime-root",
                    "/runtime",
                    "--learning-directory",
                    "/state/learning",
                    "unexpected",
                ]
                .map(OsString::from),
            )
            .is_err()
        );
    }
}
