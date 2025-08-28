use serde::{Deserialize, Serialize};
use std::process::Command;
use std::{env, fs};

#[cfg(test)]
use crate::util::hash_string;

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct SystemTag {
    pub username: String,
    pub hostname: String,
    pub os_name: String,
    pub os_version: String,
    pub architecture: String,
    pub logical_cores: usize,
}

impl SystemTag {
    pub(crate) fn from_system() -> std::io::Result<Self> {
        let username = env::var("USER").unwrap_or_else(|_| "unknown".into());

        let os_name = env::consts::OS.to_string();

        let hostname = if os_name == "macos" {
            Command::new("scutil")
                .arg("--get")
                .arg("ComputerName")
                .output()
                .map_or_else(
                    |_| "unknown".to_string(),
                    |output| {
                        if output.status.success() {
                            String::from_utf8_lossy(&output.stdout).trim().to_string()
                        } else {
                            "unknown".to_string()
                        }
                    },
                )
        } else {
            fs::read_to_string("/etc/hostname")
                .or_else(|_| fs::read_to_string("/proc/sys/kernel/hostname"))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "unknown".to_string())
        };

        let os_version = if os_name == "macos" {
            Command::new("sw_vers")
                .arg("--productVersion")
                .output()
                .map_or_else(
                    |_| "unknown".to_string(),
                    |output| {
                        if output.status.success() {
                            String::from_utf8_lossy(&output.stdout).trim().to_string()
                        } else {
                            "unknown".to_string()
                        }
                    },
                )
        } else {
            fs::read_to_string("/etc/os-release")
                .ok()
                .and_then(|content| {
                    content
                        .lines()
                        .find(|line| line.starts_with("VERSION_ID="))
                        .map(|line| {
                            line.replace("VERSION_ID=", "")
                                .replace('"', "")
                                .trim()
                                .to_string()
                        })
                })
                .unwrap_or_else(|| "unknown".to_string())
        };

        let architecture = env::consts::ARCH.to_string();

        let logical_cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);

        Ok(Self {
            username,
            hostname,
            os_name,
            os_version,
            architecture,
            logical_cores,
        })
    }

    #[cfg(test)]
    pub fn to_hash(&self) -> String {
        let json = serde_json::to_string(self).expect("Unexpected");
        hash_string(&json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_tag_json_a() {
        let original = SystemTag {
            username: "testuser".to_string(),
            hostname: "testhost".to_string(),
            os_name: "linux".to_string(),
            os_version: "5.10.0".to_string(),
            architecture: "x86_64".to_string(),
            logical_cores: 8,
        };

        let json = serde_json::to_string(&original).expect("Serialization failed");
        assert_eq!(json, "{\"username\":\"testuser\",\"hostname\":\"testhost\",\"os_name\":\"linux\",\"os_version\":\"5.10.0\",\"architecture\":\"x86_64\",\"logical_cores\":8}");

        let deserialized: SystemTag =
            serde_json::from_str(&json).expect("Deserialization failed");

        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_system_tag_hash_a() {
        let tag = SystemTag {
            username: "testuser".to_string(),
            hostname: "testhost".to_string(),
            os_name: "linux".to_string(),
            os_version: "5.10.0".to_string(),
            architecture: "x86_64".to_string(),
            logical_cores: 8,
        };

        let hash1 = tag.to_hash();
        let hash2 = tag.to_hash();
        assert_eq!(hash1, hash2);
        assert_eq!(
            hash1,
            "cfd43732505b2af92e28dc350f1f59ecdd000e5f03dd9ecb5fef250c7b4dcf53"
        )
    }

    #[test]
    fn test_system_tag_live_a() {
        let st = SystemTag::from_system();
        println!("{:?}", st);
    }
}
