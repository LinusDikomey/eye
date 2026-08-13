use std::{fmt, str::FromStr};

#[derive(Clone)]
pub struct Target {
    pub arch: Arch,
    pub vendor: Option<String>,
    pub os: Os,
    pub os2: Option<String>,
}
impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-", self.arch)?;
        if let Some(vendor) = &self.vendor {
            write!(f, "{vendor}-")?;
        }
        write!(f, "{}", self.os)?;
        if let Some(os2) = &self.os2 {
            write!(f, "-{os2}")?;
        }
        Ok(())
    }
}
impl Target {
    pub fn native() -> Result<Self, UnknownNativeTargetError> {
        Ok(Self {
            arch: native_arch(),
            vendor: None,
            os: OS.ok_or(UnknownNativeTargetError)?,
            os2: None,
        })
    }
}
impl FromStr for Target {
    type Err = &'static str;

    fn from_str(triple: &str) -> Result<Self, &'static str> {
        let (arch, rest) = triple
            .split_once("-")
            .ok_or("Target triple needs at least two components i.e. arch-os")?;
        let arch = Arch::new(arch);
        Ok(if let Some((vendor_or_os, rest)) = rest.split_once("-") {
            if let Some((os_a, os2)) = rest.split_once("-") {
                // full-form 4-part target triple
                return Ok(Target {
                    arch,
                    vendor: Some(vendor_or_os.to_owned()),
                    os: os_a.parse()?,
                    os2: Some(os2.to_owned()),
                });
            }
            // 3-part triple with either arch-vendor-os or a two-part os. Try parsing the os first
            // or assume the former otherwise
            if let Ok(os) = vendor_or_os.parse::<Os>() {
                // omitted vendor, two-part os
                return Ok(Target {
                    arch,
                    vendor: None,
                    os,
                    os2: Some(rest.to_owned()),
                });
            }
            // 3-part triple: arch-vendor-os
            Target {
                arch,
                vendor: Some(vendor_or_os.to_owned()),
                os: rest.parse()?,
                os2: None,
            }
        } else {
            // 2-part triple: arch-os
            Target {
                arch,
                vendor: None,
                os: rest.parse()?,
                os2: None,
            }
        })
    }
}

#[derive(Debug)]
pub struct UnknownNativeTargetError;

/// Supports an `Other` variant so that the architecture can just be passed through to LLVM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
    Other(String),
}
impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Arch::X86_64 => write!(f, "x86_64"),
            Arch::Aarch64 => write!(f, "aarch64"),
            Arch::Other(other) => write!(f, "{other}"),
        }
    }
}
impl Arch {
    pub fn new(s: &str) -> Self {
        match s {
            "x86_64" => Self::X86_64,
            "aarch64" => Self::Aarch64,
            _ => Self::Other(s.to_owned()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Unknown,
    Linux,
    Windows,
    Darwin,
}
impl fmt::Display for Os {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Unknown => "unknown",
                Self::Linux => "linux",
                Self::Windows => "windows",
                Self::Darwin => "darwin",
            }
        )
    }
}
impl FromStr for Os {
    type Err = UnknownOsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "linux" => Self::Linux,
            "windows" => Self::Windows,
            "darwin" => Self::Darwin,
            _ => return Err(UnknownOsError),
        })
    }
}
pub struct UnknownOsError;
impl From<UnknownOsError> for &'static str {
    fn from(_: UnknownOsError) -> Self {
        "unknown or unsupported operating system in target triple"
    }
}

#[cfg(target_os = "linux")]
const OS: Option<Os> = Some(Os::Linux);

#[cfg(target_os = "windows")]
const OS: Option<Os> = Some(Os::Windows);

#[cfg(target_os = "macos")]
const OS: Option<Os> = Some(Os::Darwin);

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
const OS: Option<Os> = None;

fn native_arch() -> Arch {
    cfg_select! {
        target_arch = "x86_64" => Arch::X86_64,
        target_arch = "aarch64" => Arch::Aarch64,
        _ => Arch::Other(std::env::consts::ARCH.to_owned()),
    }
}
