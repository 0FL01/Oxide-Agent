//! Skill loader for markdown files with YAML frontmatter.

use crate::agent::skills::types::{
    count_tokens, ActivationMode, LazyContent, Skill, SkillMetadata, SkillWeight,
};
use crate::agent::skills::{SkillError, SkillResult};
use chrono::Utc;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::warn;

/// Loads skill metadata and content from disk.
#[derive(Debug, Clone)]
pub struct SkillLoader {
    skills_dir: PathBuf,
}

impl SkillLoader {
    /// Create a new loader rooted at the skills directory.
    #[must_use]
    pub fn new(skills_dir: PathBuf) -> Self {
        Self { skills_dir }
    }

    /// Load metadata from all markdown files in the skills directory.
    pub fn load_all_metadata(&self) -> SkillResult<Vec<SkillMetadata>> {
        let entries =
            std::fs::read_dir(&self.skills_dir).map_err(|source| SkillError::ReadSkillFile {
                path: self.skills_dir.clone(),
                source,
            })?;

        let mut metadata = Vec::new();

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    warn!(error = %err, "Failed to read skill directory entry");
                    continue;
                }
            };

            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                continue;
            }

            match self.parse_file(&path) {
                Ok((meta, _)) => metadata.push(meta),
                Err(err) => {
                    warn!(path = ?path, error = %err, "Skipping invalid skill definition");
                }
            }
        }

        metadata.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(metadata)
    }

    /// Load a full skill definition by name (stem without extension).
    pub fn load_skill_content(&self, name: &str) -> SkillResult<Skill> {
        let path = self.skills_dir.join(format!("{name}.md"));
        if !path.exists() {
            return Err(SkillError::SkillNotFound(name.to_string()));
        }

        let (metadata, content) = self.parse_file(&path)?;
        let mut supporting_files = HashMap::new();

        for reference in &metadata.references {
            let resolved = if reference.is_absolute() {
                reference.clone()
            } else {
                self.skills_dir.join(reference)
            };
            supporting_files.insert(resolved.clone(), LazyContent::new(resolved));
        }

        Ok(Skill {
            metadata,
            content: content.clone(),
            supporting_files,
            loaded_at: Utc::now(),
            token_count: count_tokens(&content),
        })
    }

    fn parse_file(&self, path: &Path) -> SkillResult<(SkillMetadata, String)> {
        let raw = std::fs::read_to_string(path).map_err(|source| SkillError::ReadSkillFile {
            path: path.to_path_buf(),
            source,
        })?;

        let (frontmatter, body) = split_frontmatter(path, &raw)?;
        let file_stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(ToString::to_string);

        let metadata = frontmatter.into_metadata(path, file_stem)?;

        Ok((metadata, body))
    }
}

#[derive(Debug, Deserialize, Default)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
    triggers: Option<Vec<String>>,
    allowed_tools: Option<Vec<String>>,
    weight: Option<SkillWeight>,
    references: Option<Vec<String>>,
    activation: Option<ActivationMode>,
}

impl SkillFrontmatter {
    fn into_metadata(self, path: &Path, file_stem: Option<String>) -> SkillResult<SkillMetadata> {
        let name = self
            .name
            .or(file_stem)
            .ok_or_else(|| SkillError::MissingSkillName {
                path: path.to_path_buf(),
            })?;

        Ok(SkillMetadata {
            name,
            description: self.description.unwrap_or_default(),
            triggers: self.triggers.unwrap_or_default(),
            allowed_tools: self.allowed_tools.unwrap_or_default(),
            weight: self.weight.unwrap_or_default(),
            references: self
                .references
                .unwrap_or_default()
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            activation: self.activation.unwrap_or_default(),
            embedding: None,
        })
    }
}

fn split_frontmatter(path: &Path, content: &str) -> SkillResult<(SkillFrontmatter, String)> {
    let mut lines = content.lines();
    let first_line = lines.next().ok_or_else(|| SkillError::MissingFrontmatter {
        path: path.to_path_buf(),
    })?;

    if first_line.trim() != "---" {
        return Err(SkillError::MissingFrontmatter {
            path: path.to_path_buf(),
        });
    }

    let mut yaml_lines = Vec::new();
    let mut found_end = false;

    for line in &mut lines {
        if line.trim() == "---" {
            found_end = true;
            break;
        }
        yaml_lines.push(line);
    }

    if !found_end {
        return Err(SkillError::MissingFrontmatter {
            path: path.to_path_buf(),
        });
    }

    let yaml = yaml_lines.join("\n");
    let frontmatter: SkillFrontmatter =
        serde_yaml::from_str(&yaml).map_err(|source| SkillError::FrontmatterYaml {
            path: path.to_path_buf(),
            source,
        })?;

    let body = lines.collect::<Vec<_>>().join("\n");

    Ok((frontmatter, body.trim().to_string()))
}
