use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum BwrapNetworkMode {
    Host,
    None,
}

impl fmt::Display for BwrapNetworkMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Host => "host",
            Self::None => "none",
        })
    }
}

impl FromStr for BwrapNetworkMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "host" => Ok(Self::Host),
            "none" => Ok(Self::None),
            invalid => Err(anyhow!(
                "Invalid BWRAP_NET='{invalid}'. Valid values: host, none."
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum BwrapRootMode {
    ReadOnly,
    OverlayRw,
}

impl fmt::Display for BwrapRootMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ReadOnly => "ro",
            Self::OverlayRw => "overlay-rw",
        })
    }
}

impl FromStr for BwrapRootMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "ro" => Ok(Self::ReadOnly),
            "overlay-rw" => Ok(Self::OverlayRw),
            "tmp-overlay" => Err(anyhow!(
                "BWRAP_ROOT_MODE=tmp-overlay is not supported in the MVP. Valid values: overlay-rw, ro."
            )),
            invalid => Err(anyhow!(
                "Invalid BWRAP_ROOT_MODE='{invalid}'. Valid values: overlay-rw, ro."
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) enum BwrapResolvConf {
    Auto,
    None,
    Path(PathBuf),
}
