use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ScrollAnchor {
    Top,
    Bottom,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ScrollBehavior {
    AutoScroll,
    Manual,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum StreamAlign {
    Start,
    End,
    Center,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreamTetra {
    pub input_anchor: ScrollAnchor,
    pub scroll_behavior: ScrollBehavior,
    pub alignment: StreamAlign,
}

impl Default for StreamTetra {
    fn default() -> Self {
        Self {
            input_anchor: ScrollAnchor::Bottom,
            scroll_behavior: ScrollBehavior::AutoScroll,
            alignment: StreamAlign::Start,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TetraNode {
    Matrix, // Future MatrixTetra (Sidebar)
    Stream(StreamTetra), // Structuring Comms
    Empty,  // Placeholder
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceTetra {
    pub left_pane: TetraNode,
    pub right_pane: TetraNode,
    pub split_ratio: f32,
}

impl Default for WorkspaceTetra {
    fn default() -> Self {
        Self {
            left_pane: TetraNode::Matrix,
            right_pane: TetraNode::Stream(StreamTetra::default()),
            split_ratio: 0.25,
        }
    }
}

impl WorkspaceTetra {
    pub fn from_state(state: &bandy::state::WorkspaceState) -> Self {
        let left_pane = match &state.left_pane {
            bandy::state::ViewEntity::Topology(_) => TetraNode::Matrix,
            bandy::state::ViewEntity::Stream(s) => TetraNode::Stream(StreamTetra {
                input_anchor: match s.input_anchor {
                    bandy::state::ScrollAnchor::Top => ScrollAnchor::Top,
                    bandy::state::ScrollAnchor::Bottom => ScrollAnchor::Bottom,
                },
                scroll_behavior: match s.scroll_behavior {
                    bandy::state::ScrollBehavior::AutoScroll => ScrollBehavior::AutoScroll,
                    bandy::state::ScrollBehavior::Manual => ScrollBehavior::Manual,
                },
                alignment: match s.alignment {
                    bandy::state::StreamAlign::Start => StreamAlign::Start,
                    bandy::state::StreamAlign::End => StreamAlign::End,
                    bandy::state::StreamAlign::Center => StreamAlign::Center,
                },
            }),
            bandy::state::ViewEntity::Empty => TetraNode::Empty,
        };

        let right_pane = match &state.right_pane {
            bandy::state::ViewEntity::Topology(_) => TetraNode::Matrix,
            bandy::state::ViewEntity::Stream(s) => TetraNode::Stream(StreamTetra {
                input_anchor: match s.input_anchor {
                    bandy::state::ScrollAnchor::Top => ScrollAnchor::Top,
                    bandy::state::ScrollAnchor::Bottom => ScrollAnchor::Bottom,
                },
                scroll_behavior: match s.scroll_behavior {
                    bandy::state::ScrollBehavior::AutoScroll => ScrollBehavior::AutoScroll,
                    bandy::state::ScrollBehavior::Manual => ScrollBehavior::Manual,
                },
                alignment: match s.alignment {
                    bandy::state::StreamAlign::Start => StreamAlign::Start,
                    bandy::state::StreamAlign::End => StreamAlign::End,
                    bandy::state::StreamAlign::Center => StreamAlign::Center,
                },
            }),
            bandy::state::ViewEntity::Empty => TetraNode::Empty,
        };

        Self {
            left_pane,
            right_pane,
            split_ratio: state.split_ratio,
        }
    }
}
