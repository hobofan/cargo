use std::io::SeekFrom;
use std::io::prelude::*;
use std::path::{Path, PathBuf};
use std::process::Command;

use core::PackageId;
use hex::ToHex;
use sources::registry::{RegistryData, RegistryConfig};
use sources::registry::local::{LocalRegistry};
use util::FileLock;
use util::{Config, Sha256, Filesystem};
use util::errors::{CargoResult, CargoResultExt};

pub struct IPFSRegistry<'cfg> {
    ipfs_path: PathBuf,
    local_root: Filesystem,
    config: &'cfg Config,
    local_registry: LocalRegistry<'cfg>,
}

impl<'cfg> IPFSRegistry<'cfg> {
    pub fn new(ipfs_path: &Path,
               config: &'cfg Config,
               name: &str) -> IPFSRegistry<'cfg> {
        let local_root = config.registry_ipfs_path().join(name);
        IPFSRegistry {
            ipfs_path: ipfs_path.to_owned(),
            local_root: local_root.clone(),
            config: config,
            local_registry: LocalRegistry::new(&local_root.into_path_unlocked(), config, name),
        }
    }
}

impl<'cfg> RegistryData for IPFSRegistry<'cfg> {
    fn index_path(&self) -> &Filesystem {
        self.local_registry.index_path()
    }

    fn load(&self,
            root: &Path,
            path: &Path,
            data: &mut FnMut(&[u8]) -> CargoResult<()>) -> CargoResult<()> {
        self.local_registry.load(root, path, data)
    }

    fn config(&mut self) -> CargoResult<Option<RegistryConfig>> {
        // Local registries don't have configuration for remote APIs or anything
        // like that
        Ok(None)
    }

    fn update_index(&mut self) -> CargoResult<()> {
        // TODO: force update for ipns
        let temp_path = self.local_root.clone().into_path_unlocked().join("index").clone();
        let local_path = temp_path.to_string_lossy().clone();

        let output = Command::new("ipget")
                     .arg(self.ipfs_path.join("index").clone())
                     .args(&["-o", &local_path])
                     .output()
                     .expect("failed to execute process");

        debug!("ipget output: {:?}", output);

        // Verify if it matches the expectations of a local registry
        self.local_registry.update_index()
    }

    fn download(&mut self, pkg: &PackageId, checksum: &str)
                -> CargoResult<FileLock> {
        let filename = format!("{}-{}.crate", pkg.name(), pkg.version());
        let path = Path::new(&filename);

        // Check if crate is already downloaded.
        if let Ok(dst) = self.local_root.open_ro(path, self.config, &filename) {
            let meta = dst.file().metadata()?;
            if meta.len() > 0 {
                return Ok(dst)
            }
        }
        let mut dst = self.local_root.open_rw(path, self.config, &filename)?;
        let meta = dst.file().metadata()?;
        if meta.len() > 0 {
            return Ok(dst)
        }

        // Crate not there. Downloading it from IPFS
        self.config.shell().status("Retrieving from IPFS", pkg)?;
        let temp_path = self.local_root.clone().into_path_unlocked().join(path).clone();
        let local_path = temp_path.to_string_lossy().clone();
        let output = Command::new("ipget")
                     .arg(self.ipfs_path.join(path).clone())
                     .args(&["-o", &local_path])
                     .output()
                     .expect("failed to execute process");

        debug!("ipget output: {:?}", output);

        // Verify checksum; Somewhat redundant for IPFS, but helps ensure that ipget fully downloaded the file
        self.config.shell().status("Unpacking", pkg)?;
        let mut state = Sha256::new();
        let mut buf = [0; 64 * 1024];
        loop {
            let n = dst.read(&mut buf).chain_err(|| {
                format!("failed to read `{}`", dst.path().display())
            })?;
            if n == 0 {
                break
            }
            state.update(&buf[..n]);
        }
        if state.finish().to_hex() != checksum {
            bail!("failed to verify the checksum of `{}`", pkg)
        }

        dst.seek(SeekFrom::Start(0))?;
        Ok(dst)
    }
}
