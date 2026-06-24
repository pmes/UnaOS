use serde::{Deserialize, Serialize};

/// The fundamental GUI node parsed from the DSL blueprint.
/// We use strict `serde(tag = "node_type", content = "props")` to flawlessly map
/// the dynamic JSON representation into a strongly-typed Rust AST at compile time.
/// This approach ensures no arbitrary attributes can sneak past parsing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "node_type", content = "props")]
pub enum Node {
    WindowFrame(WindowFrameProps),
    Iterator(IteratorProps),
    Label(LabelProps),
}

/// Represents a standard window container.
/// It contains a unique `id` for identification in the blueprint,
/// and a vector of `Node` elements acting as its children.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowFrameProps {
    pub id: String,
    pub children: Vec<Node>,
}

/// A dynamic repeater node mapped directly to an external data signal.
/// `bind` designates the data source, `filter` conditionally includes elements,
/// and `item_template` forms the structure spawned for every valid item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IteratorProps {
    pub bind: String,
    pub filter: Option<String>,
    pub item_template: Box<Node>,
}

/// A simple text label node.
/// `value` expects a string template which may include variables
/// evaluated via bindings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelProps {
    pub value: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_blueprint() {
        let json_payload = r#"{
          "node_type": "WindowFrame",
          "props": {
            "id": "history_dump_window",
            "children": [
              {
                "node_type": "Iterator",
                "props": {
                  "bind": "app_state.history",
                  "filter": "is_chat == true",
                  "item_template": {
                    "node_type": "Label",
                    "props": {
                      "value": "[{{timestamp}}] {{display_name}}: {{content}}"
                    }
                  }
                }
              }
            ]
          }
        }"#;

        // Parse the raw JSON literal blueprint into our strict Rust AST.
        let node: Node = serde_json::from_str(json_payload).expect("Failed to deserialize the JSON blueprint");

        // Assert the exact structure based on our deserialized type.
        match node {
            Node::WindowFrame(window) => {
                assert_eq!(window.id, "history_dump_window");
                assert_eq!(window.children.len(), 1);

                match &window.children[0] {
                    Node::Iterator(iter) => {
                        assert_eq!(iter.bind, "app_state.history");
                        assert_eq!(iter.filter, Some("is_chat == true".to_string()));

                        match &*iter.item_template {
                            Node::Label(label) => {
                                assert_eq!(label.value, "[{{timestamp}}] {{display_name}}: {{content}}");
                            }
                            _ => panic!("Expected Label node in item_template"),
                        }
                    }
                    _ => panic!("Expected Iterator node in children"),
                }
            }
            _ => panic!("Expected WindowFrame node at root"),
        }
    }
}
