use anyhow::Context;
use cargo_about::{
    licenses::{config::Config, Gatherer, KrateLicense, LicenseInfo},
    Krates,
};
use clap::Parser;
use json_nav::json_nav;
use krates::{LockOptions, Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use spdx::{expression::ExprNode, Expression};
use std::{fs::File, io::BufReader, process::exit, sync::Arc};
use cargo_about::licenses::LicenseStore;
use tracing::metadata::LevelFilter;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
struct Opts {
    crate_manifest_dir: Utf8PathBuf,
}

#[derive(Serialize, Deserialize)]
struct LicenseFile {
    name: String,
    spdx: Option<String>,
    text: String,
}

#[derive(Serialize, Deserialize)]
struct Package {
    package_name: String,
    package_version: String,
    package_url: Option<String>,
    license_spdx: Option<String>,
    license_files: Vec<LicenseFile>,
}

fn run(Opts { crate_manifest_dir }: Opts) -> anyhow::Result<()> {
    let config = read_config()?;

    let krates = get_all_crates(&crate_manifest_dir, &config).context("Unable to get crates")?;

    let s = Arc::new(cargo_about::licenses::store_from_cache()?);

    let mut packages = vec![];
    collect_rust_licenses(&config, s.clone(), &krates, &mut packages).context("Unable to collect rust licenses")?;
    collect_cpp_licenses(s.clone(), &krates, &mut packages).context("Unable to collect cpp licenses")?;

    minimize(&config, &mut packages)?;

    let output = serde_json::to_string_pretty(&packages).context("Unable to serialize to json")?;

    println!("{output}");
    Ok(())
}

fn main() {
    let opts = Opts::parse();

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::WARN.into())
                .from_env_lossy(),
        )
        .init();

    if let Err(e) = run(opts) {
        tracing::error!("{e:#}");
        exit(1);
    }
}

fn read_config() -> anyhow::Result<Config> {
    let config_str = std::fs::read_to_string("about.toml").context("Unable to read config file")?;
    let config = toml::from_str(&config_str).context("Unable to parse config file")?;

    Ok(config)
}

fn get_all_crates(crate_manifest_dir: &Utf8Path, config: &Config) -> anyhow::Result<Krates> {
    cargo_about::get_all_crates(
        &crate_manifest_dir.join("Cargo.toml"),
        false,
        false,
        vec!["alpine-build".to_owned()],
        false,
        LockOptions { offline: false, frozen: false, locked: true },
        config,
        &[],
    )
    .context("Unable to get crates")
}

fn collect_rust_licenses(config: &Config, license_store: Arc<LicenseStore>, krates: &Krates, packages: &mut Vec<Package>) -> anyhow::Result<()> {
    let g = Gatherer::with_store(license_store);
    let c = reqwest::blocking::Client::new();

    for KrateLicense { krate, lic_info, license_files } in g.gather(&krates, config, Some(c)) {
        if krate.name.contains("tentris") {
            // ignore tentris crates
            // they are all proprietary
            continue;
        }

        match &lic_info {
            LicenseInfo::Expr(expr) => {
                let n_spdx_licenses = expr.iter().filter(|node| matches!(node, ExprNode::Req(_))).count();

                if n_spdx_licenses != license_files.len() {
                    tracing::warn!("Mismatch between license SPDX and number of license files found in crate '{}'. SPDX specifies {} but found {}", krate, n_spdx_licenses, license_files.len());
                }
            },
            LicenseInfo::Unknown => {
                tracing::warn!("crate '{}' has unknown license", krate);
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
                Err(e) => tracing::warn!("Unable to read license file {}: {e:#}", license_path),
            }
        }

        if lfiles.is_empty() {
            tracing::warn!("Unable to find any license files for {}", krate);
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

        packages.push(package);
    }

    Ok(())
}

fn collect_cpp_licenses(license_store: Arc<LicenseStore>, krates: &Krates, thirdparty: &mut Vec<Package>) -> anyhow::Result<()> {
    let tentris_crates = krates.krates().filter_map(|k| {
        let thirdparty_name = json_nav! {
            k.metadata => "tentris" => "thirdparty-file-name"; as str
        }
        .ok()?;

        Some((k, thirdparty_name.to_owned()))
    });

    for (k, thirdparty_file_name) in tentris_crates {
        let manifest_dir = k.manifest_path.parent().context("Unable to determine tentris_sys manifest dir")?;

        let thirdparty_path = manifest_dir.join(thirdparty_file_name);

        let rdr = BufReader::new(File::open(thirdparty_path).context("Unable to read thirdparty file")?);
        let mut thirdparty_packages: Vec<Package> =
            serde_json::from_reader(rdr).context("Unable to read and parse thirdparty file")?;

        for pkg in &mut thirdparty_packages {
            for l in &mut pkg.license_files {
                if l.spdx.is_none() {
                    let text = l.text.as_str().into();
                    let analysis = license_store.analyze(&text);

                    if analysis.score < 0.9 {
                        tracing::warn!("Low confidence of {} on C++ license SPDX detection for '{} {}'", analysis.score, pkg.package_name, pkg.package_version);
                    }

                    l.spdx = Some(analysis.name.to_owned());
                }
            }
        }

        thirdparty.extend(thirdparty_packages);
    }

    Ok(())
}

fn minimize(config: &Config, packages: &mut Vec<Package>) -> anyhow::Result<()> {
    for p in packages {
        if let Some(lspdx) = &p.license_spdx {
            let license_expr = Expression::parse(lspdx)?;
            let minimized_strs: Vec<_> = license_expr
                .minimized_requirements(&config.accepted)
                .with_context(|| format!("Unable to minimize requirements of '{} {}' with {:?}", p.package_name, p.package_version, p.license_spdx))?
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
