//! Private, token-free agent health record for the local student UI.
//! A running process is NOT proof that a screen is being streamed.
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentHealth {
    pub pid: u32,
    pub phase: String,
    pub updated_unix: u64,
    pub last_frame_unix: Option<u64>,
    pub backend: Option<String>,
    pub screen: Option<[u32; 2]>,
    pub last_error: Option<String>,
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn status_path() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| home.map(|p| p.join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    state.join("oko").join("agent-health.json")
}

impl AgentHealth {
    pub fn new() -> Self {
        Self {
            pid: std::process::id(),
            phase: "starting".to_string(),
            updated_unix: unix_now(),
            last_frame_unix: None,
            backend: None,
            screen: None,
            last_error: None,
        }
    }

    pub fn set(&mut self, phase: &str) {
        self.phase = phase.to_owned();
        self.updated_unix = unix_now();
        self.save_logged();
    }

    pub fn error(&mut self, message: &str) {
        self.phase = if self.phase == "connected" {
            "capture_error".into()
        } else {
            "reconnecting".into()
        };
        self.last_error = Some(message.chars().take(500).collect());
        self.updated_unix = unix_now();
        self.save_logged();
    }

    pub fn capture_ready(&mut self, backend: &str, screen: [u32; 2]) {
        self.backend = Some(backend.into());
        self.screen = Some(screen);
        self.phase = "capturing".into();
        self.updated_unix = unix_now();
        self.save_logged();
    }

    pub fn frame_sent(&mut self) {
        self.phase = "streaming".into();
        self.last_error = None;
        self.last_frame_unix = Some(unix_now());
        self.updated_unix = unix_now();
        self.save_logged();
    }

    /// Atomically replace the previous record; never include passwords or tokens.
    pub fn save(&self) -> io::Result<()> {
        let dest = status_path();
        let dir = dest.parent().expect("status file has parent");
        fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        let temp = dir.join(format!(".agent-health-{}.tmp", self.pid));
        let mut opts = OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(&temp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        let serialized = serde_json::to_vec(self).map_err(io::Error::other)?;
        file.write_all(&serialized)?;
        file.sync_all()?;
        fs::rename(temp, dest)?;
        Ok(())
    }

    pub fn save_logged(&self) {
        if let Err(e) = self.save() {
            eprintln!("[oko-agent] unable to save private health status: {e}");
        }
    }
}

impl Default for AgentHealth {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn health_serialization_does_not_include_secrets() {
        let mut status = AgentHealth::new();
        status.capture_ready("x11", [1280, 800]);
        let text = serde_json::to_string(&status).unwrap();
        assert!(text.contains("\"capturing\""));
        assert!(text.contains("\"x11\""));
        assert!(!text.contains("OKO_TOKEN"));
        assert_eq!(
            serde_json::from_str::<AgentHealth>(&text).unwrap().pid,
            status.pid
        );
    }
}
