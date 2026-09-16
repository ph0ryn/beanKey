use std::error::Error;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;

use beankey_daemon::{DaemonConfig, DaemonServer, Engine, ServerError};

fn main() {
    if let Err(error) = run() {
        eprintln!("beankey-daemon: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let config_path = parse_config_argument()?;
    let config = DaemonConfig::load(config_path)?;
    let learning_directory =
        state_home(std::env::var_os("XDG_STATE_HOME"), std::env::var_os("HOME"))?
            .join("beankey/learning");
    let engine = Engine::open_with_config(&config, learning_directory)?;
    let server = match DaemonServer::bind_from_environment(engine, &config.runtime_socket) {
        Ok(server) => server,
        Err(ServerError::AlreadyRunning) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    server.run()?;
    Ok(())
}

fn state_home(xdg_state_home: Option<OsString>, home: Option<OsString>) -> Result<PathBuf, String> {
    if let Some(value) = xdg_state_home.filter(|value| !value.is_empty()) {
        let path = PathBuf::from(value);
        return absolute_directory(path, "XDG_STATE_HOME");
    }
    let home = home.ok_or_else(|| "HOME is required when XDG_STATE_HOME is unset".to_owned())?;
    let home = absolute_directory(PathBuf::from(home), "HOME")?;
    Ok(home.join(".local/state"))
}

fn absolute_directory(path: PathBuf, variable: &str) -> Result<PathBuf, String> {
    if Path::new(&path).is_absolute() {
        Ok(path)
    } else {
        Err(format!("{variable} must be an absolute path"))
    }
}

fn parse_config_argument() -> Result<PathBuf, Box<dyn Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let Some(flag) = arguments.next() else {
        return Err("usage: beankey-daemon --config PATH".into());
    };
    if flag != "--config" {
        return Err("usage: beankey-daemon --config PATH".into());
    }
    let Some(path) = arguments.next() else {
        return Err("--config requires a path".into());
    };
    if arguments.next().is_some() {
        return Err("unexpected daemon arguments".into());
    }
    Ok(path.into())
}

#[cfg(test)]
mod tests {
    use super::state_home;
    use std::ffi::OsString;
    use std::path::Path;

    #[test]
    fn resolves_the_xdg_state_home_and_its_documented_fallback() {
        assert_eq!(
            state_home(
                Some(OsString::from("/state")),
                Some(OsString::from("/home/user"))
            )
            .unwrap(),
            Path::new("/state")
        );
        assert_eq!(
            state_home(None, Some(OsString::from("/home/user"))).unwrap(),
            Path::new("/home/user/.local/state")
        );
        assert!(state_home(Some(OsString::from("relative")), None).is_err());
    }
}
