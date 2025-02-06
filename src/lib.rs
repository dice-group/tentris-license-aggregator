use anyhow::Context;
use cargo_about::{
    licenses::{Gatherer, KrateLicense, LicenseInfo},
    Krates,
};
use krates::{LockOptions};
use serde::{Deserialize, Serialize};
use spdx::{expression::ExprNode, Expression};
use std::sync::Arc;

pub use krates::{Utf8Path, Utf8PathBuf};
pub use cargo_about::licenses::{config::Config, LicenseStore};

#[derive(Serialize, Deserialize)]
pub struct LicenseFile {
    /// Filename of the license file
    pub name: String,
    /// If known, the SPDX identifier of the license
    pub spdx: Option<String>,
    /// The content of the license file
    pub text: String,
}

#[derive(Serialize, Deserialize)]
pub struct Package {
    /// Name of the package
    pub package_name: String,
    /// Version of the package
    pub package_version: String,
    /// Url of the package (this might be the repository or the crates.io page or a homepage)
    pub package_url: Option<String>,
    /// If known, the combined SPDX expression for all licenses of the package (e.g. MIT OR Apache-2.0)
    pub license_spdx: Option<String>,
    /// All the license files that couldd be found for the package
    pub license_files: Vec<LicenseFile>,
}

/// Create a license store from an internal cache
pub fn license_store_from_cache() -> anyhow::Result<Arc<LicenseStore>> {
    Ok(Arc::new(cargo_about::licenses::store_from_cache()?))
}

/// Retrieve all rust packages and their licenses based on the Cargo.toml at the given path
pub fn get_all_licenses<P: AsRef<Utf8Path>>(
    cargo_toml: P,
    features: Vec<String>,
    license_store: Arc<LicenseStore>,
    config: &Config,
) -> anyhow::Result<Vec<Package>> {
    let krates = cargo_about::get_all_crates(
        cargo_toml.as_ref(),
        false,
        false,
        features,
        false,
        LockOptions { offline: false, frozen: false, locked: true },
        config,
        &[],
    )
    .context("Unable to get crates")?;

    collect_krate_licenses(&krates, license_store, config)
}

/// If the SPDX identifier of individual licenses in the packages are unknown
/// use the license store to analyze the license contents to determine their SPDX.
pub fn augment_licenses(licenses: &mut [Package], license_store: Arc<LicenseStore>) -> anyhow::Result<()> {
    for pkg in licenses {
        for l in &mut pkg.license_files {
            if l.spdx.is_none() {
                let text = l.text.as_str().into();
                let analysis = license_store.analyze(&text);

                if analysis.score < 0.9 {
                    tracing::warn!(
                        "Low confidence of {} on C++ license SPDX detection for '{} {}'",
                        analysis.score,
                        pkg.package_name,
                        pkg.package_version
                    );
                }

                l.spdx = Some(analysis.name.to_owned());
            }
        }
    }

    Ok(())
}

/// Minimize the license requirements for the packages, based on preferences in the configuration
///
/// # Example
/// `MIT OR Apache-2.0` may be minimized to just `MIT`
pub fn minimize_requirements(packages: &mut [Package], config: &Config) -> anyhow::Result<()> {
    for p in packages {
        if let Some(lspdx) = &p.license_spdx {
            let license_expr = Expression::parse(lspdx)?;
            let minimized_strs: Vec<_> = license_expr
                .minimized_requirements(&config.accepted)
                .with_context(|| {
                    format!(
                        "Unable to minimize requirements of '{} {}' with {:?}",
                        p.package_name, p.package_version, p.license_spdx
                    )
                })?
                .into_iter()
                .map(|req| req.to_string())
                .collect();

            p.license_files.retain(|license| {
                license.spdx.is_none() || license.spdx.as_ref().is_some_and(|spdx| minimized_strs.contains(spdx))
            })
        }
    }

    Ok(())
}

fn collect_krate_licenses(
    krates: &Krates,
    license_store: Arc<LicenseStore>,
    config: &Config,
) -> anyhow::Result<Vec<Package>> {
    let g = Gatherer::with_store(license_store);
    let c = reqwest::blocking::Client::new();

    let mut packages = Vec::new();

    for KrateLicense { krate, lic_info, license_files } in g.gather(krates, config, Some(c)) {
        if krate.name.contains("tentris") {
            // ignore tentris crates
            // they are all proprietary
            continue;
        }

        match &lic_info {
            LicenseInfo::Expr(expr) => {
                let n_spdx_licenses = expr.iter().filter(|node| matches!(node, ExprNode::Req(_))).count();

                if n_spdx_licenses != license_files.len() {
                    tracing::warn!("Mismatch between license SPDX and number of license files found in crate '{krate}'. SPDX specifies {n_spdx_licenses} but found {}", license_files.len());
                }
            },
            LicenseInfo::Unknown => {
                tracing::warn!("crate '{krate}' has unknown license");
            },
            LicenseInfo::Ignore => {
                anyhow::bail!("Ignoring a crate shouldd not happen");
            },
        }

        let mut lfiles = vec![];
        for l in license_files {
            let license_path = if l.path.is_absolute() {
                l.path.to_owned()
            } else {
                krate.manifest_path.parent().unwrap().join(l.path)
            };

            let name = license_path.file_name().unwrap().to_owned();
            match std::fs::read_to_string(&license_path) {
                Ok(text) => lfiles.push(LicenseFile { name, spdx: Some(l.license_expr.to_string()), text }),
                Err(e) => tracing::warn!("Unable to read license file {license_path}: {e:#}"),
            }
        }

        if lfiles.is_empty() {
            tracing::warn!("Unable to find any license files for {krate}");
        }

        let package = Package {
            package_name: krate.name.clone(),
            package_version: krate.version.to_string(),
            package_url: krate
                .repository
                .as_ref()
                .or(krate.homepage.as_ref())
                .map(ToOwned::to_owned),
            license_spdx: Some(lic_info.to_string()),
            license_files: lfiles,
        };

        packages.extend(std::iter::once(package));
    }

    Ok(packages)
}
