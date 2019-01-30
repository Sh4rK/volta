//! Provides the `Session` type, which represents the user's state during an
//! execution of a Notion tool, including their current directory, Notion
//! hook configuration, and the state of the local inventory.

use std::rc::Rc;

use crate::distro::{DistroVersion, Fetched};
use crate::error::ErrorDetails;
use crate::hook::{HookConfig, LazyHookConfig, Publish};
use crate::inventory::{Inventory, LazyInventory};
// use crate::package::PackageInfo;
use crate::package::PackageDistro;
use crate::platform::PlatformSpec;
use crate::project::{LazyProject, Project};
use crate::tool::ToolSpec;
use crate::toolchain::LazyToolchain;
use crate::version::VersionSpec;

use std::fmt::{self, Display, Formatter};
use std::process::exit;

use crate::event::EventLog;
use notion_fail::{throw, ExitCode, Fallible, NotionError};
use semver::Version;

#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Copy)]
pub enum ActivityKind {
    Fetch,
    Install,
    Uninstall,
    Current,
    Deactivate,
    Activate,
    Default,
    Pin,
    Node,
    Npm,
    Npx,
    Yarn,
    Notion,
    Tool,
    Help,
    Version,
    Binary,
    Shim,
    Completions,
    Which,
}

impl Display for ActivityKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        let s = match self {
            &ActivityKind::Fetch => "fetch",
            &ActivityKind::Install => "install",
            &ActivityKind::Uninstall => "uninstall",
            &ActivityKind::Current => "current",
            &ActivityKind::Deactivate => "deactivate",
            &ActivityKind::Activate => "activate",
            &ActivityKind::Default => "default",
            &ActivityKind::Pin => "pin",
            &ActivityKind::Node => "node",
            &ActivityKind::Npm => "npm",
            &ActivityKind::Npx => "npx",
            &ActivityKind::Yarn => "yarn",
            &ActivityKind::Notion => "notion",
            &ActivityKind::Tool => "tool",
            &ActivityKind::Help => "help",
            &ActivityKind::Version => "version",
            &ActivityKind::Binary => "binary",
            &ActivityKind::Shim => "shim",
            &ActivityKind::Completions => "completions",
            &ActivityKind::Which => "which",
        };
        f.write_str(s)
    }
}

/// Represents the user's state during an execution of a Notion tool. The session
/// encapsulates a number of aspects of the environment in which the tool was
/// invoked, including:
///     - the current directory
///     - the Node project tree that contains the current directory (if any)
///     - the Notion hook configuration
///     - the inventory of locally-fetched Notion tools
pub struct Session {
    hooks: LazyHookConfig,
    inventory: LazyInventory,
    toolchain: LazyToolchain,
    project: LazyProject,
    event_log: EventLog,
}

impl Session {
    /// Constructs a new `Session`.
    pub fn new() -> Session {
        Session {
            hooks: LazyHookConfig::new(),
            inventory: LazyInventory::new(),
            toolchain: LazyToolchain::new(),
            project: LazyProject::new(),
            event_log: EventLog::new(),
        }
    }

    /// Produces a reference to the current Node project, if any.
    pub fn project(&self) -> Fallible<Option<Rc<Project>>> {
        self.project.get()
    }

    pub fn current_platform(&self) -> Fallible<Option<Rc<PlatformSpec>>> {
        match self.project_platform()? {
            Some(platform) => Ok(Some(platform)),
            None => self.user_platform(),
        }
    }

    pub fn user_platform(&self) -> Fallible<Option<Rc<PlatformSpec>>> {
        let toolchain = self.toolchain.get()?;
        Ok(toolchain
            .platform_ref()
            .map(|platform| Rc::new(platform.clone())))
    }

    /// Returns the current project's pinned platform image, if any.
    pub fn project_platform(&self) -> Fallible<Option<Rc<PlatformSpec>>> {
        if let Some(ref project) = self.project()? {
            return Ok(project.platform());
        }
        Ok(None)
    }

    /// Produces a reference to the current inventory.
    pub fn inventory(&self) -> Fallible<&Inventory> {
        self.inventory.get()
    }

    /// Produces a mutable reference to the current inventory.
    pub fn inventory_mut(&mut self) -> Fallible<&mut Inventory> {
        self.inventory.get_mut()
    }

    /// Produces a reference to the hook configuration
    pub fn hooks(&self) -> Fallible<&HookConfig> {
        self.hooks.get()
    }

    /// Ensures that a specific Node version has been fetched and unpacked
    pub(crate) fn ensure_node(&mut self, version: &Version) -> Fallible<()> {
        let inventory = self.inventory.get_mut()?;

        if !inventory.node.contains(version) {
            let hooks = self.hooks.get()?;
            inventory.fetch(&ToolSpec::Node(VersionSpec::exact(version)), hooks)?;
        }

        Ok(())
    }

    /// Ensures that a specific Yarn version has been fetched and unpacked
    pub(crate) fn ensure_yarn(&mut self, version: &Version) -> Fallible<()> {
        let inventory = self.inventory.get_mut()?;

        if !inventory.yarn.contains(version) {
            let hooks = self.hooks.get()?;
            inventory.fetch(&ToolSpec::Yarn(VersionSpec::exact(version)), hooks)?;
        }

        Ok(())
    }

    /// Installs a Tool matching the specified semantic versioning requirements,
    /// and updates the `toolchain` as necessary.
    // TODO: should this be match-ed back in the install command?
    pub fn install(&mut self, toolspec: &ToolSpec) -> Fallible<()> {
        match toolspec {
            ToolSpec::Node(_version) => self.install_distro(toolspec),
            ToolSpec::Yarn(_version) => self.install_distro(toolspec),
            // ISSUE (#175) implement as part of fetching packages
            ToolSpec::Npm(_) => unimplemented!("cannot install npm, yet"),
            // TODO: this should use toolspec as well (or refactor all of this stuff)
            ToolSpec::Package(name, version) => self.install_package(&name, &version),
        }
    }

    // TODO: this should be better parameterized for type checking (AKA no ToolSpec enum)
    fn install_distro(&mut self, toolspec: &ToolSpec) -> Fallible<()> {
        let distro_version = self.fetch_distro(toolspec)?.into_version();
        let toolchain = self.toolchain.get_mut()?;
        toolchain.set_active(distro_version)?;
        Ok(())
    }

    // TODO: description, and use toolspec
    fn install_package(&mut self, name: &String, version: &VersionSpec) -> Fallible<()> {
        // fetches and unpacks package
        let package_distro = self.fetch_package(name, version)?;

        let use_platform;

        if let Some(platform) = PackageDistro::platform(package_distro.version())? {
            println!("using 'platform' from the package: {:?}", platform);
            use_platform = platform;
        } else if let Some(platform) = self.user_platform()? {
            println!("using user platform: {:?}", platform);
            use_platform = platform;
        } else {
            println!("no package platform or user platform - using node latest");
            let latest_spec = ToolSpec::Node(VersionSpec::Latest);
            let distro_version = self.fetch_distro(&latest_spec)?.into_version();

            // TODO: dammit, the fucking DV again, really got to fix the typing
            if let DistroVersion::Node(node, npm) = distro_version {
                use_platform = Rc::new(PlatformSpec {
                    node_runtime: node,
                    npm: Some(npm),
                    yarn: None,
                });
                println!("using platform: {:?}", use_platform);
            } else {
                // TODO: this is to avoid unintialized use_platform
                // take this out after fixing the typing
                println!("SHOULDN'T GET HERE - IF IT DOES ====> BIG PROBLEMS");
                use_platform = Rc::new(PlatformSpec {
                    node_runtime: Version::parse("1.2.3").unknown()?,
                    npm: None,
                    yarn: None,
                });
                println!("using platform: {:?}", use_platform);
            }
            // TODO: is that all I need to do for that?
        }

        // finally, install the package
        PackageDistro::install(&package_distro.version(), &use_platform, self)
    }

    /// Fetches a Tool version matching the specified semantic versioning requirements.
    pub fn fetch_distro(&mut self, tool: &ToolSpec) -> Fallible<Fetched<DistroVersion>> {
        let inventory = self.inventory.get_mut()?;
        let hooks = self.hooks.get()?;
        inventory.fetch(&tool, hooks)
    }

    /// Fetches a Packge version matching the specified semantic versioning requirements.
    pub fn fetch_package(&mut self, name: &String, version: &VersionSpec) -> Fallible<Fetched<DistroVersion>> {
        let inventory = self.inventory.get_mut()?;
        let config = self.config.get()?;
        inventory.fetch_package(name, version, config)
    }

    /// Updates toolchain in package.json with the Tool version matching the specified semantic
    /// versioning requirements.
    // TODO: match things in the command, so this is parameterized for type checking?
    pub fn pin(&mut self, toolspec: &ToolSpec) -> Fallible<()> {
        if let Some(ref project) = self.project()? {
            let distro_version = self.fetch_distro(toolspec)?.into_version();
            project.pin(&distro_version)?;
        } else {
            throw!(ErrorDetails::NotInPackage);
        }
        Ok(())
    }

    pub fn add_event_start(&mut self, activity_kind: ActivityKind) {
        self.event_log.add_event_start(activity_kind)
    }
    pub fn add_event_end(&mut self, activity_kind: ActivityKind, exit_code: ExitCode) {
        self.event_log.add_event_end(activity_kind, exit_code)
    }
    pub fn add_event_tool_end(&mut self, activity_kind: ActivityKind, exit_code: i32) {
        self.event_log.add_event_tool_end(activity_kind, exit_code)
    }
    pub fn add_event_error(&mut self, activity_kind: ActivityKind, error: &NotionError) {
        self.event_log.add_event_error(activity_kind, error)
    }

    fn publish_to_event_log(mut self) {
        match publish_plugin(&self.hooks) {
            Ok(plugin) => {
                self.event_log.publish(plugin);
            }
            Err(e) => {
                eprintln!("Warning: invalid config file ({})", e);
            }
        }
    }

    pub fn exit(self, code: ExitCode) -> ! {
        self.publish_to_event_log();
        code.exit();
    }

    pub fn exit_tool(self, code: i32) -> ! {
        self.publish_to_event_log();
        exit(code);
    }
}

fn publish_plugin(hooks: &LazyHookConfig) -> Fallible<Option<&Publish>> {
    let hooks = hooks.get()?;
    Ok(hooks
        .events
        .as_ref()
        .and_then(|events| events.publish.as_ref()))
}

#[cfg(test)]
pub mod tests {

    use crate::session::Session;
    use std::env;
    use std::path::PathBuf;

    fn fixture_path(fixture_dir: &str) -> PathBuf {
        let mut cargo_manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        cargo_manifest_dir.push("fixtures");
        cargo_manifest_dir.push(fixture_dir);
        cargo_manifest_dir
    }

    #[test]
    fn test_in_pinned_project() {
        let project_pinned = fixture_path("basic");
        env::set_current_dir(&project_pinned).expect("Could not set current directory");
        let pinned_session = Session::new();
        let pinned_platform = pinned_session
            .project_platform()
            .expect("Couldn't create Project");
        assert_eq!(pinned_platform.is_some(), true);

        let project_unpinned = fixture_path("no_toolchain");
        env::set_current_dir(&project_unpinned).expect("Could not set current directory");
        let unpinned_session = Session::new();
        let unpinned_platform = unpinned_session
            .project_platform()
            .expect("Couldn't create Project");
        assert_eq!(unpinned_platform.is_none(), true);
    }
}
