use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use bstr::{BStr, BString, ByteSlice};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RepoPath(pub BString);

impl RepoPath {
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(BString::from(bytes))
    }

    pub fn from_str(value: &str) -> Self {
        Self(BString::from(value.as_bytes().to_vec()))
    }

    pub fn as_bstr(&self) -> &BStr {
        self.0.as_bstr()
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_slice()
    }

    pub fn display_lossy(&self) -> String {
        self.0.to_str_lossy().into_owned()
    }

    pub fn join_to(&self, root: &Path) -> PathBuf {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;

            return root.join(OsStr::from_bytes(self.as_bytes()));
        }
        #[cfg(not(unix))]
        {
            root.join(self.display_lossy())
        }
    }

    pub fn to_os_string(&self) -> OsString {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;

            return OsString::from_vec(self.as_bytes().to_vec());
        }
        #[cfg(not(unix))]
        {
            OsString::from(self.display_lossy())
        }
    }
}

impl From<&str> for RepoPath {
    fn from(value: &str) -> Self {
        Self::from_str(value)
    }
}
