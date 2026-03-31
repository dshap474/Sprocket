use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct HermeticEnv {
    pub home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub codex_home: PathBuf,
    pub cargo_home: PathBuf,
}

impl HermeticEnv {
    pub fn new(root: &std::path::Path) -> Self {
        Self {
            home: root.join("home"),
            xdg_config_home: root.join("xdg-config"),
            codex_home: root.join("codex-home"),
            cargo_home: root.join("cargo-home"),
        }
    }

    pub fn pairs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("HOME", self.home.display().to_string()),
            (
                "XDG_CONFIG_HOME",
                self.xdg_config_home.display().to_string(),
            ),
            ("CODEX_HOME", self.codex_home.display().to_string()),
            ("CARGO_HOME", self.cargo_home.display().to_string()),
            ("GIT_CONFIG_NOSYSTEM", "1".to_string()),
        ]
    }

    pub fn ensure_dirs(&self) {
        for dir in [
            &self.home,
            &self.xdg_config_home,
            &self.codex_home,
            &self.cargo_home,
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
    }
}
