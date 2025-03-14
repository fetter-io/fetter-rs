use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::process::Command;

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct SystemTag {
    username: String,
    hostname: String,
    os_name: String,
    os_version: String,
    architecture: String,
    logical_cores: usize,
}

impl SystemTag {
    pub fn from_system() -> std::io::Result<Self> {
        let username = env::var("USER").unwrap_or_else(|_| "unknown".into());

        let hostname = fs::read_to_string("/etc/hostname")
            .or_else(|_| fs::read_to_string("/proc/sys/kernel/hostname"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        let os_name = env::consts::OS.to_string();

        let os_version = if os_name == "macos" {
            Command::new("sw_vers")
                .arg("-productVersion")
                .output()
                .ok()
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            fs::read_to_string("/proc/version")
                .map(|s| s.split_whitespace().nth(2).unwrap_or("unknown").to_string())
                .unwrap_or_else(|_| "unknown".to_string())
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn test_json_serialization_round_trip() {
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
}
