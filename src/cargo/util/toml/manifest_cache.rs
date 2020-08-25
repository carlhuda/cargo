use std::collections::{
    hash_map::{Entry, HashMap},
    BTreeSet,
};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::{parse, TomlManifest};
use crate::util::errors::{CargoResult, CargoResultExt, ManifestError};
use crate::util::{paths, Config};

pub type ManifestCache = HashMap<PathBuf, Rc<ParseOutput>>;

#[derive(Debug)]
pub struct ParseOutput {
    pub manifest: Rc<TomlManifest>,
    pub unused: BTreeSet<String>,
}

pub fn parse_manifest<'a>(
    manifest_file: &'_ Path,
    config: &'a Config,
) -> Result<Rc<ParseOutput>, ManifestError> {
    let key = manifest_file.parent().unwrap().to_path_buf();
    let mut cache = config.manifest_cache();

    match cache.entry(key.clone()) {
        Entry::Occupied(e) => Ok(Rc::clone(e.get())),
        Entry::Vacant(v) => {
            let contents = paths::read(manifest_file)
                .map_err(|err| ManifestError::new(err, manifest_file.into()))?;

            let output = deserialize(contents, manifest_file, config)
                .chain_err(|| format!("failed to parse manifest at `{}`", manifest_file.display()))
                .map_err(|err| ManifestError::new(err, manifest_file.into()))?;

            Ok(Rc::clone(v.insert(Rc::new(output))))
        }
    }
}

fn deserialize(
    contents: String,
    manifest_file: &Path,
    config: &Config,
) -> CargoResult<ParseOutput> {
    let pretty_filename = manifest_file
        .strip_prefix(config.cwd())
        .unwrap_or(manifest_file);

    let toml = parse(&contents, pretty_filename, config)?;
    let mut unused = BTreeSet::new();
    let manifest: TomlManifest = serde_ignored::deserialize(toml, |path| {
        let mut key = String::new();
        stringify(&mut key, &path);
        unused.insert(key);
    })?;

    Ok(ParseOutput {
        manifest: Rc::new(manifest),
        unused,
    })
}

fn stringify(dst: &mut String, path: &serde_ignored::Path<'_>) {
    use serde_ignored::Path;

    match *path {
        Path::Root => {}
        Path::Seq { parent, index } => {
            stringify(dst, parent);
            if !dst.is_empty() {
                dst.push('.');
            }
            dst.push_str(&index.to_string());
        }
        Path::Map { parent, ref key } => {
            stringify(dst, parent);
            if !dst.is_empty() {
                dst.push('.');
            }
            dst.push_str(key);
        }
        Path::Some { parent }
        | Path::NewtypeVariant { parent }
        | Path::NewtypeStruct { parent } => stringify(dst, parent),
    }
}
