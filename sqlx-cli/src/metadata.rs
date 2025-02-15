use anyhow::{Context, Result};
use cargo_metadata::{
    Metadata as CargoMetadata, Package as MetadataPackage, PackageId as MetadataId,
};

use std::{
    collections::{btree_map, BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    str::FromStr,
};

/// The minimal amount of package information we care about
///
/// The package's `name` is used to `cargo clean -p` specific crates while the `src_paths` are
/// are used to trigger recompiles of packages within the workspace
#[derive(Debug)]
pub struct Package {
    name: String,
    src_paths: Vec<PathBuf>,
}

impl Package {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn src_paths(&self) -> &[PathBuf] {
        &self.src_paths
    }
}

impl From<&MetadataPackage> for Package {
    fn from(package: &MetadataPackage) -> Self {
        let name = package.name.clone();
        let src_paths = package
            .targets
            .iter()
            .map(|target| target.src_path.clone().into_std_path_buf())
            .collect();

        Self { name, src_paths }
    }
}

/// Contains metadata for the current project
pub struct Metadata {
    /// Maps packages metadata id to the package
    ///
    /// Currently `MetadataId` is used over `PkgId` because pkgid is not a UUID
    packages: BTreeMap<MetadataId, Package>,
    /// All of the crates in the current workspace
    workspace_members: Vec<MetadataId>,
    /// Maps each dependency to its set of dependents
    reverse_deps: BTreeMap<MetadataId, BTreeSet<MetadataId>>,
    /// The target directory of the project
    ///
    /// Typically `target` at the workspace root, but can be overridden
    target_directory: PathBuf,
}

impl Metadata {
    pub fn package(&self, id: &MetadataId) -> Option<&Package> {
        self.packages.get(id)
    }

    pub fn entries<'this>(&'this self) -> btree_map::Iter<'this, MetadataId, Package> {
        self.packages.iter()
    }

    pub fn workspace_members(&self) -> &[MetadataId] {
        &self.workspace_members
    }

    pub fn target_directory(&self) -> &Path {
        &self.target_directory
    }

    /// Gets all dependents (direct and transitive) of `id`
    pub fn all_dependents_of(&self, id: &MetadataId) -> BTreeSet<&MetadataId> {
        let mut dependents = BTreeSet::new();
        self.all_dependents_of_helper(id, &mut dependents);
        dependents
    }

    fn all_dependents_of_helper<'this>(
        &'this self,
        id: &MetadataId,
        dependents: &mut BTreeSet<&'this MetadataId>,
    ) {
        if let Some(immediate_dependents) = self.reverse_deps.get(id) {
            for immediate_dependent in immediate_dependents {
                if dependents.insert(immediate_dependent) {
                    self.all_dependents_of_helper(&immediate_dependent, dependents);
                }
            }
        }
    }
}

impl FromStr for Metadata {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let CargoMetadata {
            packages: metadata_packages,
            workspace_members,
            resolve,
            target_directory,
            ..
        } = serde_json::from_str(s)?;

        let mut packages = BTreeMap::new();
        for metadata_package in metadata_packages {
            let package = Package::from(&metadata_package);
            packages.insert(metadata_package.id, package);
        }

        let mut reverse_deps: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        let resolve =
            resolve.context("Resolving the dependency graph failed (old version of cargo)")?;
        for node in resolve.nodes {
            for dep in node.deps {
                let dependent = node.id.clone();
                let dependency = dep.pkg;
                reverse_deps
                    .entry(dependency)
                    .or_default()
                    .insert(dependent);
            }
        }

        let target_directory = target_directory.into_std_path_buf();

        Ok(Self {
            packages,
            workspace_members,
            reverse_deps,
            target_directory,
        })
    }
}
