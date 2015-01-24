use std::cmp;
use std::fmt::{self, Show, Formatter};
use std::io::fs::{self, PathExtensions};
use glob::Pattern;
use git2;

use core::{Package, PackageId, Summary, SourceId, Source, Dependency, Registry};
use ops;
use util::{CargoResult, internal, internal_error, human, ChainError, Config};

pub struct PathSource<'a, 'b: 'a> {
    id: SourceId,
    path: Path,
    updated: bool,
    packages: Vec<Package>,
    config: &'a Config<'b>,
}

// TODO: Figure out if packages should be discovered in new or self should be
// mut and packages are discovered in update
impl<'a, 'b> PathSource<'a, 'b> {
    pub fn for_path(path: &Path, config: &'a Config<'b>)
                    -> CargoResult<PathSource<'a, 'b>> {
        log!(5, "PathSource::for_path; path={}", path.display());
        Ok(PathSource::new(path, &try!(SourceId::for_path(path)), config))
    }

    /// Invoked with an absolute path to a directory that contains a Cargo.toml.
    /// The source will read the manifest and find any other packages contained
    /// in the directory structure reachable by the root manifest.
    pub fn new(path: &Path, id: &SourceId, config: &'a Config<'b>)
               -> PathSource<'a, 'b> {
        log!(5, "new; id={}", id);

        PathSource {
            id: id.clone(),
            path: path.clone(),
            updated: false,
            packages: Vec::new(),
            config: config,
        }
    }

    pub fn get_root_package(&self) -> CargoResult<Package> {
        log!(5, "get_root_package; source={:?}", self);

        if !self.updated {
            return Err(internal("source has not been updated"))
        }

        match self.packages.iter().find(|p| p.get_root() == self.path) {
            Some(pkg) => Ok(pkg.clone()),
            None => Err(internal("no package found in source"))
        }
    }

    pub fn read_packages(&self) -> CargoResult<Vec<Package>> {
        if self.updated {
            Ok(self.packages.clone())
        } else {
            ops::read_packages(&self.path, &self.id, self.config)
        }
    }

    /// List all files relevant to building this package inside this source.
    ///
    /// This function will use the appropriate methods to determine what is the
    /// set of files underneath this source's directory which are relevant for
    /// building `pkg`.
    ///
    /// The basic assumption of this method is that all files in the directory
    /// are relevant for building this package, but it also contains logic to
    /// use other methods like .gitignore to filter the list of files.
    pub fn list_files(&self, pkg: &Package) -> CargoResult<Vec<Path>> {
        let root = pkg.get_manifest_path().dir_path();

        let exclude = pkg.get_manifest().get_exclude().iter().map(|p| {
            Pattern::new(p.as_slice())
        }).collect::<Vec<Pattern>>();
        let include = pkg.get_manifest().get_include().iter().map(|p| {
            Pattern::new(p.as_slice())
        }).collect::<Vec<Pattern>>();

        let mut filter = |&mut: p: &Path| {
            let relative_path = p.path_relative_from(&root).unwrap();
            include.iter().any(|p| p.matches_path(&relative_path)) || {
                include.len() == 0 &&
                 !exclude.iter().any(|p| p.matches_path(&relative_path))
            }
        };

        // If this package is a git repository, then we really do want to query
        // the git repository as it takes into account items such as .gitignore.
        // We're not quite sure where the git repository is, however, so we do a
        // bit of a probe.
        //
        // We check all packages in this source that are ancestors of the
        // specified package (including the same package) to see if they're at
        // the root of the git repository. This isn't always true, but it'll get
        // us there most of the time!.
        let repo = self.packages.iter()
                       .map(|pkg| pkg.get_root())
                       .filter(|path| path.is_ancestor_of(&root))
                       .filter_map(|path| git2::Repository::open(&path).ok())
                       .next();
        match repo {
            Some(repo) => self.list_files_git(pkg, repo, &mut filter),
            None => self.list_files_walk(pkg, filter),
        }
    }

    fn list_files_git<F>(&self, pkg: &Package, repo: git2::Repository,
                         filter: &mut F)
                         -> CargoResult<Vec<Path>>
        where F: FnMut(&Path) -> bool
    {
        warn!("list_files_git {}", pkg.get_package_id());
        let index = try!(repo.index());
        let root = try!(repo.workdir().chain_error(|| {
            internal_error("Can't list files on a bare repository.", "")
        }));
        let pkg_path = pkg.get_manifest_path().dir_path();

        let mut ret = Vec::new();

        // We use information from the git repository to guide use in traversing
        // its tree. The primary purpose of this is to take advantage of the
        // .gitignore and auto-ignore files that don't matter.
        //
        // If the repository has no commits, then we check the status of all
        // files (tracked and untracked) and use all those. If the repository
        // has at least one commit, however, we assume that all relevant files
        // are in the index so we filter out all the statuses.
        let index_files = index.iter().map(|entry| root.join(&entry.path[]));
        let mut opts = git2::StatusOptions::new();
        let unborn = repo.head().is_err();
        opts.include_untracked(true);
        let statuses = try!(repo.statuses(Some(&mut opts)));
        let untracked = statuses.iter().map(|entry| root.join(entry.path_bytes()))
                                .filter(|_| unborn);

        'outer: for file_path in index_files.chain(untracked) {
            // Filter out files outside this package.
            if !pkg_path.is_ancestor_of(&file_path) { continue }

            // Filter out Cargo.lock and target always
            if file_path.filename() == Some(b"Cargo.lock") { continue }
            if file_path.filename() == Some(b"target") { continue }

            // Filter out sub-packages of this package
            for other_pkg in self.packages.iter().filter(|p| *p != pkg) {
                let other_path = other_pkg.get_manifest_path().dir_path();
                if pkg_path.is_ancestor_of(&other_path) &&
                   other_path.is_ancestor_of(&file_path) {
                    continue 'outer;
                }
            }

            // TODO: the `entry` has a mode we should be able to look at instead
            //       of just calling stat() again
            if file_path.is_dir() {
                warn!("  found submodule {}", file_path.display());
                let rel = file_path.path_relative_from(&root).unwrap();
                let rel = try!(rel.as_str().chain_error(|| {
                    human(format!("invalid utf-8 filename: {}", rel.display()))
                }));
                match repo.find_submodule(rel) {
                    Ok(submodule) => {
                        let repo = try!(submodule.open());
                        let files = try!(self.list_files_git(pkg, repo, filter));
                        ret.extend(files.into_iter());
                    }
                    Err(..) => {
                        try!(self.walk(&file_path, &mut ret, false, filter));
                    }
                }
            } else if (*filter)(&file_path) {
                // We found a file!
                warn!("  found {}", file_path.display());
                ret.push(file_path);
            }
        }
        Ok(ret)
    }

    fn list_files_walk<F>(&self, pkg: &Package, mut filter: F)
                          -> CargoResult<Vec<Path>>
        where F: FnMut(&Path) -> bool
    {
        let mut ret = Vec::new();
        for pkg in self.packages.iter().filter(|p| *p == pkg) {
            let loc = pkg.get_manifest_path().dir_path();
            try!(self.walk(&loc, &mut ret, true, &mut filter));
        }
        return Ok(ret);

    }

    fn walk<F>(&self, path: &Path, ret: &mut Vec<Path>,
               is_root: bool, filter: &mut F) -> CargoResult<()>
        where F: FnMut(&Path) -> bool
    {
        if !path.is_dir() {
            if (*filter)(path) {
                ret.push(path.clone());
            }
            return Ok(())
        }
        // Don't recurse into any sub-packages that we have
        if !is_root && path.join("Cargo.toml").exists() { return Ok(()) }
        for dir in try!(fs::readdir(path)).iter() {
            match (is_root, dir.filename_str()) {
                (_,    Some(".git")) |
                (true, Some("target")) |
                (true, Some("Cargo.lock")) => continue,
                _ => {}
            }
            try!(self.walk(dir, ret, false, filter));
        }
        return Ok(())
    }
}

impl<'a, 'b> Show for PathSource<'a, 'b> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "the paths source")
    }
}

impl<'a, 'b> Registry for PathSource<'a, 'b> {
    fn query(&mut self, dep: &Dependency) -> CargoResult<Vec<Summary>> {
        let mut summaries: Vec<Summary> = self.packages.iter()
                                              .map(|p| p.get_summary().clone())
                                              .collect();
        summaries.query(dep)
    }
}

impl<'a, 'b> Source for PathSource<'a, 'b> {
    fn update(&mut self) -> CargoResult<()> {
        if !self.updated {
            let packages = try!(self.read_packages());
            self.packages.extend(packages.into_iter());
            self.updated = true;
        }

        Ok(())
    }

    fn download(&mut self, _: &[PackageId])  -> CargoResult<()>{
        // TODO: assert! that the PackageId is contained by the source
        Ok(())
    }

    fn get(&self, ids: &[PackageId]) -> CargoResult<Vec<Package>> {
        log!(5, "getting packages; ids={:?}", ids);

        Ok(self.packages.iter()
           .filter(|pkg| ids.iter().any(|id| pkg.get_package_id() == id))
           .map(|pkg| pkg.clone())
           .collect())
    }

    fn fingerprint(&self, pkg: &Package) -> CargoResult<String> {
        if !self.updated {
            return Err(internal_error("BUG: source was not updated", ""));
        }

        let mut max = 0;
        for file in try!(self.list_files(pkg)).iter() {
            // An fs::stat error here is either because path is a
            // broken symlink, a permissions error, or a race
            // condition where this path was rm'ed - either way,
            // we can ignore the error and treat the path's mtime
            // as 0.
            warn!("{} {}", file.stat().map(|s| s.modified).unwrap_or(0), file.display());
            max = cmp::max(max, file.stat().map(|s| s.modified).unwrap_or(0));
        }
        log!(5, "fingerprint {}: {}", self.path.display(), max);
        Ok(max.to_string())
    }
}
