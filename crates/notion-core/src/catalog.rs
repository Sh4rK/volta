use std::path::Path;
use std::fs::File;
use std::io::Write;
use std::str::FromStr;

use toml::value::{Value, Table};

use version::Version;
use path::user_catalog_file;
use untoml::{ParseToml, Extract, load};
use install;
use failure;

use super::CatalogError;

fn toml_error(key: String) -> CatalogError {
    CatalogError {
        msg: format!("invalid catalog file at key '{}'", key)
    }
}

pub struct Catalog {
    pub node: Option<Version>
}

pub fn catalog() -> Result<Catalog, failure::Error> {
    let path = user_catalog_file()?;
    load(&path)
}

fn save(path: &Path, catalog: &Catalog) -> Result<(), failure::Error> {
    let mut file = File::create(path)?;
    if let Some(Version::Public(ref version)) = catalog.node {
        file.write_all(b"[node]\n")?;
        file.write_fmt(format_args!("version = \"{}\"\n", version))?;
    }
    Ok(())
}

pub fn set(version: Version) -> Result<(), failure::Error> {
    {
        let &Version::Public(ref version) = &version;
        install::by_version(version)?;
    }
    let path = user_catalog_file()?;
    let mut catalog: Catalog = load(&path)?;
    catalog.node = Some(version);
    save(&path, &catalog)?;
    Ok(())
}

fn parse_node_version(root: &mut Table) -> Result<Option<Version>, failure::Error> {
    if !root.contains_key("node") {
        return Ok(None);
    }
    let mut node = root.extract("node", toml_error)?.table("node", toml_error)?;
    if !node.contains_key("version") {
        return Ok(None);
    }
    let version = node.extract("version", toml_error)?.string("node.version", toml_error)?;
    Ok(Some(Version::Public(version)))
}

impl FromStr for Catalog {
    type Err = failure::Error;

    fn from_str(src: &str) -> Result<Self, Self::Err> {
        let toml = src.parse::<Value>()?;
        let mut root = toml.table("<root>", toml_error)?;
        let node = parse_node_version(&mut root)?;
        Ok(Catalog { node })
    }
}