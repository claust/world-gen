use std::ffi::OsString;

use anyhow::{anyhow, Result};

#[derive(Clone, Debug)]
pub struct DebugApiConfig {
    pub enabled: bool,
    pub bind_addr: String,
}

impl Default for DebugApiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_addr: "127.0.0.1:7777".to_string(),
        }
    }
}

impl DebugApiConfig {
    pub fn from_env_args() -> Result<Self> {
        Self::from_iter(
            std::env::args_os().skip(1),
            std::env::var_os("WORLD_GEN_DEBUG_API"),
        )
    }

    fn from_iter<I>(args: I, env_debug_api: Option<OsString>) -> Result<Self>
    where
        I: IntoIterator<Item = OsString>,
    {
        let mut config = Self::default();

        if env_is_truthy(env_debug_api.as_deref()) {
            config.enabled = true;
        }

        let mut iter = args.into_iter();
        while let Some(arg) = iter.next() {
            let arg_str = arg.to_string_lossy();
            match arg_str.as_ref() {
                "--debug-api" => config.enabled = true,
                "--debug-api-bind" => {
                    let Some(value) = iter.next() else {
                        return Err(anyhow!("--debug-api-bind requires a value"));
                    };
                    config.bind_addr = value.to_string_lossy().to_string();
                }
                _ => {}
            }
        }

        Ok(config)
    }
}

fn env_is_truthy(value: Option<&std::ffi::OsStr>) -> bool {
    value
        .map(|v| {
            let lowered = v.to_string_lossy().trim().to_ascii_lowercase();
            matches!(lowered.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::DebugApiConfig;

    #[test]
    fn default_is_disabled() {
        let parsed = DebugApiConfig::from_iter(Vec::<std::ffi::OsString>::new(), None).unwrap();
        assert!(!parsed.enabled);
        assert_eq!(parsed.bind_addr, "127.0.0.1:7777");
    }

    #[test]
    fn cli_flag_enables_debug_api() {
        let args = vec![std::ffi::OsString::from("--debug-api")];
        let parsed = DebugApiConfig::from_iter(args, None).unwrap();
        assert!(parsed.enabled);
    }

    #[test]
    fn cli_bind_overrides_default() {
        let args = vec![
            std::ffi::OsString::from("--debug-api-bind"),
            std::ffi::OsString::from("127.0.0.1:9000"),
        ];
        let parsed = DebugApiConfig::from_iter(args, None).unwrap();
        assert_eq!(parsed.bind_addr, "127.0.0.1:9000");
    }

    #[test]
    fn env_enables_debug_api() {
        let parsed = DebugApiConfig::from_iter(
            Vec::<std::ffi::OsString>::new(),
            Some(std::ffi::OsString::from("true")),
        )
        .unwrap();
        assert!(parsed.enabled);
    }
}
