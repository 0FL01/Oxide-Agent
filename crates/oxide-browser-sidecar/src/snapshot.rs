//! Accessibility tree snapshot — ported from chrome-agent's `snapshot.rs`.
//!
//! Calls `Accessibility.enable` + `Accessibility.getFullAXTree` via CDP,
//! applies 4 noise-filtering rules, generates stable UIDs from
//! `backendDOMNodeId`, and returns a flat structured list matching the
//! Python sidecar's `parse_snapshot()` output format.
//!
//! The text format (`uid=nN role "name" [props]`) is also produced in
//! parallel for logging/debugging — it is NOT an intermediate representation
//! that gets parsed back; both outputs are primary.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::cdp::CdpClient;

/// CDP command timeout for accessibility operations.
const CDP_TIMEOUT: Duration = Duration::from_secs(10);

/// Noise roles that repeat parent content and waste tokens.
const NOISE_ROLES: &[&str] = &["none", "StaticText", "InlineTextBox"];

/// One node in the accessibility tree summary.
///
/// Matches the Python sidecar's `parse_snapshot()` output: `{uid, role, text, depth}`.
/// Properties (focused, disabled, etc.) are in the text format only, not here.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct A11yNode {
    pub uid: String,
    pub role: String,
    pub text: String,
    pub depth: usize,
}

/// Result of taking an a11y tree snapshot.
pub struct SnapshotResult {
    /// Flat list of visible a11y nodes (noise filtered) — goes into `a11y_summary`.
    pub nodes: Vec<A11yNode>,
    /// Human-readable text format (`uid=nN role "name" [props]`) — for logging.
    pub text: String,
    /// UID → `backendDOMNodeId` mapping for click actions (CP4).
    pub uid_to_backend: HashMap<String, i64>,
}

/// Errors from the snapshot module.
#[derive(Debug, Error)]
pub enum SnapshotError {
    /// CDP transport or protocol error.
    #[error("CDP error: {0}")]
    Cdp(String),
    /// Response missing required `nodes` field.
    #[error("missing or invalid 'nodes' field in AX tree response")]
    MissingNodes,
}

/// Take an accessibility tree snapshot of the current page.
///
/// Calls `Accessibility.enable` + `Accessibility.getFullAXTree`, applies
/// 4 noise-filtering rules, and returns structured + text output.
pub async fn take_snapshot(cdp: &CdpClient) -> Result<SnapshotResult, SnapshotError> {
    // Enable accessibility domain.
    cdp.send_command("Accessibility.enable", Value::Null, CDP_TIMEOUT)
        .await
        .map_err(|e| SnapshotError::Cdp(e.to_string()))?;

    // Fetch the full AX tree.
    let result = cdp
        .send_command("Accessibility.getFullAXTree", Value::Null, CDP_TIMEOUT)
        .await
        .map_err(|e| SnapshotError::Cdp(e.to_string()))?;

    let nodes: Vec<AxNode> = result
        .get("nodes")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .ok_or(SnapshotError::MissingNodes)?;

    Ok(format_ax_tree(&nodes))
}

// ── CDP AX types ───────────────────────────────────────────────────────

/// CDP `Accessibility.AXNode` — flat list node from `getFullAXTree`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AxNode {
    node_id: String,
    #[serde(default)]
    ignored: bool,
    #[serde(default)]
    role: Option<AxValue>,
    #[serde(default)]
    name: Option<AxValue>,
    #[serde(default)]
    value: Option<AxValue>,
    #[serde(default)]
    properties: Option<Vec<AxProperty>>,
    #[serde(default)]
    child_ids: Option<Vec<String>>,
    #[serde(default, rename = "backendDOMNodeId")]
    backend_dom_node_id: Option<i64>,
    #[serde(default)]
    parent_id: Option<String>,
}

/// CDP `Accessibility.AXValue` — typed value wrapper.
#[derive(Debug, Clone, Deserialize)]
struct AxValue {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    value_type: String,
    #[serde(default)]
    value: Option<Value>,
}

/// CDP `Accessibility.AXProperty` — named property.
#[derive(Debug, Clone, Deserialize)]
struct AxProperty {
    name: String,
    value: AxValue,
}

impl AxNode {
    /// Extract the human-readable role string.
    fn role_name(&self) -> &str {
        self.role
            .as_ref()
            .and_then(|r| r.value.as_ref())
            .and_then(|v| v.as_str())
            .unwrap_or("")
    }

    /// Extract the human-readable name string.
    fn name_value(&self) -> &str {
        self.name
            .as_ref()
            .and_then(|n| n.value.as_ref())
            .and_then(|v| v.as_str())
            .unwrap_or("")
    }

    /// Extract the value string for inputs.
    fn value_str(&self) -> &str {
        self.value
            .as_ref()
            .and_then(|v| v.value.as_ref())
            .and_then(|v| v.as_str())
            .unwrap_or("")
    }
}

// ── Tree formatting ────────────────────────────────────────────────────

/// Format the flat AX node list into a filtered, flat summary + text.
///
/// Applies 4 noise rules (ported from chrome-agent's `snapshot.rs`):
/// 1. Skip `ignored` nodes — recurse children.
/// 2. Skip roles `none`/`StaticText`/`InlineTextBox` — recurse children.
/// 3. Skip unnamed `generic` containers — recurse children.
/// 4. Pull text from `StaticText` children when node name is empty.
fn format_ax_tree(nodes: &[AxNode]) -> SnapshotResult {
    // Build lookup: nodeId → AxNode.
    let node_by_id: HashMap<&str, &AxNode> =
        nodes.iter().map(|n| (n.node_id.as_str(), n)).collect();

    // Find root (node with no parentId, or first node).
    let root_id = nodes
        .iter()
        .find(|n| n.parent_id.is_none())
        .map(|n| n.node_id.as_str());

    let Some(root_id) = root_id else {
        return SnapshotResult {
            nodes: Vec::new(),
            text: String::new(),
            uid_to_backend: HashMap::new(),
        };
    };

    let mut result = SnapshotResult {
        nodes: Vec::new(),
        text: String::new(),
        uid_to_backend: HashMap::new(),
    };
    let mut uid_counter: u32 = 0;

    format_node(root_id, &node_by_id, 0, &mut uid_counter, &mut result);

    result
}

/// Recursively format a node, applying noise rules.
///
/// Pushes structured `A11yNode` and text format line in parallel.
fn format_node(
    node_id: &str,
    nodes: &HashMap<&str, &AxNode>,
    depth: usize,
    uid_counter: &mut u32,
    result: &mut SnapshotResult,
) {
    let Some(node) = nodes.get(node_id) else {
        return;
    };

    // Rule 1: Skip ignored nodes — recurse children.
    if node.ignored {
        recurse_children(node, nodes, depth, uid_counter, result);
        return;
    }

    let role = node.role_name();
    let mut name = node.name_value().to_string();

    // Rule 2: Skip noise roles (none/StaticText/InlineTextBox) — recurse children.
    if NOISE_ROLES.contains(&role) {
        recurse_children(node, nodes, depth, uid_counter, result);
        return;
    }

    // Rule 4: Pull text from StaticText children when name is empty.
    if name.is_empty()
        && let Some(child_ids) = &node.child_ids
    {
        let texts: Vec<&str> = child_ids
            .iter()
            .filter_map(|cid| nodes.get(cid.as_str()))
            .filter(|n| n.role_name() == "StaticText")
            .filter_map(|n| {
                let nm = n.name_value();
                if nm.is_empty() { None } else { Some(nm) }
            })
            .collect();
        if !texts.is_empty() {
            name = texts.join(" ");
        }
    }

    // Rule 3: Skip unnamed generic containers — recurse children.
    if role == "generic" && name.is_empty() {
        recurse_children(node, nodes, depth, uid_counter, result);
        return;
    }

    // Assign UID — stable (backendDOMNodeId) when available, sequential fallback.
    let uid = if let Some(backend_id) = node.backend_dom_node_id {
        let uid = format!("n{backend_id}");
        result.uid_to_backend.insert(uid.clone(), backend_id);
        uid
    } else {
        *uid_counter += 1;
        format!("e{uid_counter}")
    };

    // Push structured node (no properties — matches parse_snapshot output).
    result.nodes.push(A11yNode {
        uid: uid.clone(),
        role: role.to_string(),
        text: name.clone(),
        depth,
    });

    // Push text format line (with properties — matches chrome-agent output).
    let indent = "  ".repeat(depth);
    result.text.push_str(&indent);
    result.text.push_str("uid=");
    result.text.push_str(&uid);

    if !role.is_empty() {
        result.text.push(' ');
        result.text.push_str(role);
    }

    if !name.is_empty() {
        result.text.push_str(" \"");
        result.text.push_str(&name);
        result.text.push('"');
    }

    // Value (for inputs).
    let val = node.value_str();
    if !val.is_empty() {
        result.text.push_str(" value=\"");
        result.text.push_str(val);
        result.text.push('"');
    }

    // Properties: focused, disabled, expanded, selected, level, checked, required, readonly.
    if let Some(props) = &node.properties {
        for prop in props {
            let pv = prop.value.value.as_ref();
            match prop.name.as_str() {
                "focused" if pv.and_then(Value::as_bool).unwrap_or(false) => {
                    result.text.push_str(" focused");
                }
                "disabled" if pv.and_then(Value::as_bool).unwrap_or(false) => {
                    result.text.push_str(" disabled");
                }
                "expanded" if pv.and_then(Value::as_bool).unwrap_or(false) => {
                    result.text.push_str(" expanded");
                }
                "selected" if pv.and_then(Value::as_bool).unwrap_or(false) => {
                    result.text.push_str(" selected");
                }
                "checked" => {
                    if let Some(val) = pv.and_then(Value::as_str)
                        && val != "false"
                    {
                        result.text.push_str(" checked=");
                        result.text.push_str(val);
                    }
                }
                "level" => {
                    if let Some(level) = pv.and_then(Value::as_u64) {
                        result.text.push_str(" level=");
                        result.text.push_str(&level.to_string());
                    }
                }
                "required" if pv.and_then(Value::as_bool).unwrap_or(false) => {
                    result.text.push_str(" required");
                }
                "readonly" if pv.and_then(Value::as_bool).unwrap_or(false) => {
                    result.text.push_str(" readonly");
                }
                _ => {}
            }
        }
    }

    result.text.push('\n');

    // Recurse children at increased depth.
    recurse_children(node, nodes, depth + 1, uid_counter, result);
}

/// Recurse into a node's children at the given depth.
fn recurse_children(
    node: &AxNode,
    nodes: &HashMap<&str, &AxNode>,
    depth: usize,
    uid_counter: &mut u32,
    result: &mut SnapshotResult,
) {
    if let Some(child_ids) = &node.child_ids {
        for child_id in child_ids {
            format_node(child_id, nodes, depth, uid_counter, result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ax_node(
        node_id: &str,
        role: Option<&str>,
        name: Option<&str>,
        backend_id: Option<i64>,
        child_ids: Option<Vec<&str>>,
        parent_id: Option<&str>,
        ignored: bool,
    ) -> AxNode {
        AxNode {
            node_id: node_id.to_string(),
            ignored,
            role: role.map(|r| AxValue {
                value_type: "role".to_string(),
                value: Some(json!(r)),
            }),
            name: name.map(|n| AxValue {
                value_type: "computedString".to_string(),
                value: Some(json!(n)),
            }),
            value: None,
            properties: None,
            child_ids: child_ids.map(|ids| ids.iter().map(|s| s.to_string()).collect()),
            backend_dom_node_id: backend_id,
            parent_id: parent_id.map(|s| s.to_string()),
        }
    }

    #[test]
    fn formats_simple_tree() {
        let nodes = vec![ax_node(
            "1",
            Some("heading"),
            Some("Welcome"),
            Some(10),
            Some(vec![]),
            None,
            false,
        )];
        let result = format_ax_tree(&nodes);
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nodes[0].uid, "n10");
        assert_eq!(result.nodes[0].role, "heading");
        assert_eq!(result.nodes[0].text, "Welcome");
        assert_eq!(result.nodes[0].depth, 0);
        assert_eq!(result.uid_to_backend.get("n10"), Some(&10));
        assert!(result.text.contains("uid=n10 heading \"Welcome\""));
    }

    #[test]
    fn skips_ignored_nodes_recurse_children() {
        let nodes = vec![
            ax_node("1", None, None, None, Some(vec!["2"]), None, true),
            ax_node(
                "2",
                Some("button"),
                Some("Click me"),
                Some(20),
                Some(vec![]),
                Some("1"),
                false,
            ),
        ];
        let result = format_ax_tree(&nodes);
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nodes[0].uid, "n20");
        assert_eq!(result.nodes[0].depth, 0);
        assert!(!result.text.contains("ignored"));
    }

    #[test]
    fn skips_noise_roles_recurse_children() {
        let nodes = vec![
            ax_node("1", Some("none"), None, None, Some(vec!["2"]), None, false),
            ax_node(
                "2",
                Some("button"),
                Some("Submit"),
                Some(30),
                Some(vec![]),
                Some("1"),
                false,
            ),
        ];
        let result = format_ax_tree(&nodes);
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nodes[0].uid, "n30");
        assert_eq!(result.nodes[0].text, "Submit");
    }

    #[test]
    fn skips_static_text_role_recurse_children() {
        let nodes = vec![
            ax_node(
                "1",
                Some("StaticText"),
                Some("Hello"),
                None,
                Some(vec!["2"]),
                None,
                false,
            ),
            ax_node(
                "2",
                Some("paragraph"),
                Some("World"),
                Some(31),
                Some(vec![]),
                Some("1"),
                false,
            ),
        ];
        let result = format_ax_tree(&nodes);
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nodes[0].uid, "n31");
    }

    #[test]
    fn skips_unnamed_generic_recurse_children() {
        let nodes = vec![
            ax_node(
                "1",
                Some("generic"),
                Some(""),
                Some(40),
                Some(vec!["2"]),
                None,
                false,
            ),
            ax_node(
                "2",
                Some("link"),
                Some("Home"),
                Some(41),
                Some(vec![]),
                Some("1"),
                false,
            ),
        ];
        let result = format_ax_tree(&nodes);
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nodes[0].uid, "n41");
        assert_eq!(result.nodes[0].text, "Home");
    }

    #[test]
    fn named_generic_is_kept() {
        let nodes = vec![ax_node(
            "1",
            Some("generic"),
            Some("Named section"),
            Some(60),
            Some(vec![]),
            None,
            false,
        )];
        let result = format_ax_tree(&nodes);
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nodes[0].text, "Named section");
    }

    #[test]
    fn pulls_text_from_static_text_children() {
        let nodes = vec![
            ax_node(
                "1",
                Some("button"),
                Some(""),
                Some(50),
                Some(vec!["2"]),
                None,
                false,
            ),
            ax_node(
                "2",
                Some("StaticText"),
                Some("Login"),
                None,
                Some(vec![]),
                Some("1"),
                false,
            ),
        ];
        let result = format_ax_tree(&nodes);
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nodes[0].uid, "n50");
        assert_eq!(result.nodes[0].text, "Login");
    }

    #[test]
    fn pulls_text_from_multiple_static_text_children() {
        let nodes = vec![
            ax_node(
                "1",
                Some("link"),
                Some(""),
                Some(51),
                Some(vec!["2", "3"]),
                None,
                false,
            ),
            ax_node(
                "2",
                Some("StaticText"),
                Some("Click"),
                None,
                Some(vec![]),
                Some("1"),
                false,
            ),
            ax_node(
                "3",
                Some("StaticText"),
                Some("here"),
                None,
                Some(vec![]),
                Some("1"),
                false,
            ),
        ];
        let result = format_ax_tree(&nodes);
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nodes[0].text, "Click here");
    }

    #[test]
    fn fallback_uid_for_nodes_without_backend_id() {
        let nodes = vec![ax_node(
            "1",
            Some("heading"),
            Some("Title"),
            None,
            Some(vec![]),
            None,
            false,
        )];
        let result = format_ax_tree(&nodes);
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nodes[0].uid, "e1");
        assert!(result.uid_to_backend.is_empty());
    }

    #[test]
    fn empty_tree_returns_empty() {
        let nodes: Vec<AxNode> = vec![];
        let result = format_ax_tree(&nodes);
        assert!(result.nodes.is_empty());
        assert!(result.uid_to_backend.is_empty());
        assert!(result.text.is_empty());
    }

    #[test]
    fn all_ignored_nodes_returns_empty() {
        let nodes = vec![
            ax_node("1", None, None, None, Some(vec!["2"]), None, true),
            ax_node("2", None, None, None, Some(vec![]), Some("1"), true),
        ];
        let result = format_ax_tree(&nodes);
        assert!(result.nodes.is_empty());
    }

    #[test]
    fn uid_stability_with_backend_ids() {
        // Same backendDOMNodeId → same UID across snapshots, even with different nodeIds.
        let nodes1 = vec![ax_node(
            "1",
            Some("button"),
            Some("A"),
            Some(100),
            Some(vec![]),
            None,
            false,
        )];
        let nodes2 = vec![ax_node(
            "99",
            Some("button"),
            Some("A"),
            Some(100),
            Some(vec![]),
            None,
            false,
        )];
        let r1 = format_ax_tree(&nodes1);
        let r2 = format_ax_tree(&nodes2);
        assert_eq!(r1.nodes[0].uid, r2.nodes[0].uid);
        assert_eq!(r1.nodes[0].uid, "n100");
    }

    #[test]
    fn depth_increments_correctly() {
        let nodes = vec![
            ax_node(
                "1",
                Some("WebArea"),
                Some("Page"),
                Some(1),
                Some(vec!["2"]),
                None,
                false,
            ),
            ax_node(
                "2",
                Some("heading"),
                Some("Title"),
                Some(2),
                Some(vec!["3"]),
                Some("1"),
                false,
            ),
            ax_node(
                "3",
                Some("button"),
                Some("Click"),
                Some(3),
                Some(vec![]),
                Some("2"),
                false,
            ),
        ];
        let result = format_ax_tree(&nodes);
        assert_eq!(result.nodes.len(), 3);
        assert_eq!(result.nodes[0].depth, 0); // WebArea
        assert_eq!(result.nodes[1].depth, 1); // heading
        assert_eq!(result.nodes[2].depth, 2); // button
    }

    #[test]
    fn text_format_includes_properties() {
        let mut node = ax_node(
            "1",
            Some("button"),
            Some("Login"),
            Some(11),
            Some(vec![]),
            None,
            false,
        );
        node.properties = Some(vec![
            AxProperty {
                name: "focused".to_string(),
                value: AxValue {
                    value_type: "boolean".to_string(),
                    value: Some(json!(true)),
                },
            },
            AxProperty {
                name: "disabled".to_string(),
                value: AxValue {
                    value_type: "boolean".to_string(),
                    value: Some(json!(false)),
                },
            },
        ]);
        let result = format_ax_tree(&[node]);
        assert!(result.text.contains("focused"));
        assert!(!result.text.contains("disabled"));
    }

    #[test]
    fn text_format_includes_level_for_headings() {
        let mut node = ax_node(
            "1",
            Some("heading"),
            Some("Title"),
            Some(12),
            Some(vec![]),
            None,
            false,
        );
        node.properties = Some(vec![AxProperty {
            name: "level".to_string(),
            value: AxValue {
                value_type: "integer".to_string(),
                value: Some(json!(2)),
            },
        }]);
        let result = format_ax_tree(&[node]);
        assert!(result.text.contains("level=2"));
    }

    #[test]
    fn inline_text_box_is_noise() {
        let nodes = vec![ax_node(
            "1",
            Some("InlineTextBox"),
            Some("text fragment"),
            None,
            Some(vec![]),
            None,
            false,
        )];
        let result = format_ax_tree(&nodes);
        assert!(result.nodes.is_empty());
    }
}
