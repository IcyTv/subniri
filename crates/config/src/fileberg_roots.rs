//! User-defined folder identities. Paths remain lexical, not filesystem-dependent.
use config_macros::{Config, ConfigSerialize};
use config_traits::ConfigError;
use kdl::KdlNode;

pub const FILEBERG_FOLDER_ICONS: &[&str] = &[
	"folder",
	"house",
	"desktop",
	"download-simple",
	"file-text",
	"image",
	"music-notes",
	"film-strip",
	"users",
	"copy",
	"code",
	"git-branch",
	"pen-nib",
	"briefcase",
	"smiley",
	"archive",
	"star",
	"gear",
	"package",
];

#[derive(Debug, Default, Clone, PartialEq, Eq, Config, ConfigSerialize)]
pub struct FilebergRoot {
	pub path: String,
	pub label: String,
	pub icon: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FilebergRoots(pub Vec<FilebergRoot>);

impl config_traits::Config for FilebergRoots {
	fn from_kdl_node(node: &KdlNode) -> Result<Self, ConfigError> {
		if !node.entries().is_empty() {
			return Err(ConfigError::wrong_type(Some(node.span())));
		}

		let mut roots = Vec::new();
		if let Some(children) = node.children() {
			for child in children.nodes() {
				if child.name().value() != "root" {
					return Err(ConfigError::unknown_field(
						child.name().value(),
						Some(child.span()),
					));
				}

				roots.push(FilebergRoot::from_kdl_node(child)?);
			}
		}

		Ok(Self(roots))
	}
}

impl config_traits::ConfigSerialize for FilebergRoots {
	fn apply_to_kdl_node(&self, node: &mut KdlNode) -> Result<(), ConfigError> {
		node.entries_mut().clear();
		let children = node.ensure_children();
		children.nodes_mut().clear();

		for root in &self.0 {
			let mut child = KdlNode::new("root");
			root.apply_to_kdl_node(&mut child)?;
			children.nodes_mut().push(child);
		}

		Ok(())
	}
}

pub(crate) fn validate(roots: &FilebergRoots, _: &()) -> garde::Result {
	let mut paths = std::collections::HashSet::new();
	for root in &roots.0 {
		if !valid_path(&root.path)
			|| root.label.trim().is_empty()
			|| root.label.chars().any(char::is_control)
		{
			return Err(garde::Error::new(
				"Folder roots require an absolute or ~/ path and a nonempty label",
			));
		}

		if !paths.insert(std::path::Path::new(&root.path)) {
			return Err(garde::Error::new("Duplicate folder root path"));
		}

		if !FILEBERG_FOLDER_ICONS.contains(&root.icon.as_str())
			&& !(valid_path(&root.icon) && root.icon.to_ascii_lowercase().ends_with(".svg"))
		{
			return Err(garde::Error::new(
				"Folder icon must be a built-in name or an absolute/~/ SVG path",
			));
		}
	}

	Ok(())
}

fn valid_path(value: &str) -> bool {
	!value.contains('\0')
		&& (value.starts_with('/') || value.starts_with("~/"))
		&& !std::path::Path::new(value)
			.components()
			.any(|part| part == std::path::Component::ParentDir)
}

#[cfg(test)]
mod tests {
	use super::*;
	use config_traits::{ConfigFileSerialize as _, ConfigFileValidateExt as _};

	#[test]
	fn roots_round_trip_and_clear_without_changing_other_preferences() -> Result<(), ConfigError> {
		let input = "fileberg { favorites \"copy\"; roots { root { path \"~/work\"; label \"Work\"; icon \"briefcase\"; }; }; }";
		let (mut doc, mut config) = crate::ConfigFile::parse_validated(input)?;

		assert_eq!(config.fileberg.roots.0[0].label, "Work");
		assert!(config.nix_diff(&crate::ConfigFile::default()).contains("services.subniri.settings.fileberg.roots = [{ path = \"~/work\"; label = \"Work\"; icon = \"briefcase\"; }];"));

		config.apply_to_kdl_document(&mut doc)?;
		assert_eq!(
			crate::ConfigFile::parse_validated(&doc.to_string())?
				.1
				.fileberg,
			config.fileberg
		);

		config.fileberg.roots.0.clear();
		config.apply_to_kdl_document(&mut doc)?;

		let (_, restored) = crate::ConfigFile::parse_validated(&doc.to_string())?;
		assert!(restored.fileberg.roots.0.is_empty());
		assert_eq!(restored.fileberg.favorites, ["copy"]);

		Ok(())
	}

	#[test]
	fn roots_validate_paths_icons_labels_and_duplicates() {
		let root = FilebergRoot {
			path: "~/work".into(),
			label: "Work".into(),
			icon: "code".into(),
		};

		assert!(validate(&FilebergRoots(vec![root.clone()]), &()).is_ok());
		assert!(validate(&FilebergRoots(vec![root.clone(), root.clone()]), &()).is_err());

		for (path, label, icon) in [
			("relative", "Work", "code"),
			("/a/../b", "Work", "code"),
			("/a", "", "code"),
			("/a", "A", "unknown"),
		] {
			assert!(
				validate(
					&FilebergRoots(vec![FilebergRoot {
						path: path.into(),
						label: label.into(),
						icon: icon.into()
					}]),
					&()
				)
				.is_err()
			);
		}

		assert!(
			validate(
				&FilebergRoots(vec![FilebergRoot {
					icon: "~/icons/work.svg".into(),
					..root
				}]),
				&()
			)
			.is_ok()
		);
	}
}
