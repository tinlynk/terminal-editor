use std::collections::BTreeMap;

use crate::{
    document::DocumentId,
    editor::EditorView,
    layout::{
        CalculatedLayout, FocusDirection, LayoutNode, PaneId, Rect, SplitDirection, SplitId,
        nearest_pane,
    },
};

#[derive(Clone, Debug)]
pub enum Pane {
    Empty,
    Editor {
        document_id: DocumentId,
        view: EditorView,
    },
}

#[derive(Clone, Debug)]
pub struct Workspace {
    pub layout: LayoutNode,
    pub panes: BTreeMap<PaneId, Pane>,
    pub tabs: BTreeMap<PaneId, Vec<DocumentId>>,
    pub focused: PaneId,
    next_pane_id: u64,
    next_split_id: u64,
    initial_split_ratio: f32,
}

impl Workspace {
    pub fn empty(initial_split_ratio: f32) -> Self {
        let pane_id = PaneId(1);
        Self {
            layout: LayoutNode::Pane(pane_id),
            panes: BTreeMap::from([(pane_id, Pane::Empty)]),
            tabs: BTreeMap::from([(pane_id, Vec::new())]),
            focused: pane_id,
            next_pane_id: 2,
            next_split_id: 1,
            initial_split_ratio,
        }
    }

    pub fn with_editor(document_id: DocumentId, initial_split_ratio: f32) -> Self {
        let pane_id = PaneId(1);
        Self {
            layout: LayoutNode::Pane(pane_id),
            panes: BTreeMap::from([(
                pane_id,
                Pane::Editor {
                    document_id,
                    view: EditorView::default(),
                },
            )]),
            tabs: BTreeMap::from([(pane_id, vec![document_id])]),
            focused: pane_id,
            next_pane_id: 2,
            next_split_id: 1,
            initial_split_ratio,
        }
    }

    pub fn focused_pane(&self) -> &Pane {
        &self.panes[&self.focused]
    }

    pub fn focused_pane_mut(&mut self) -> &mut Pane {
        self.panes
            .get_mut(&self.focused)
            .expect("focused pane must exist")
    }

    pub fn split_contextual(&mut self, direction: SplitDirection) -> PaneId {
        let pane = match self.focused_pane().clone() {
            Pane::Empty => Pane::Empty,
            Pane::Editor { document_id, .. } => Pane::Editor {
                document_id,
                view: EditorView::default(),
            },
        };
        self.insert_split(pane, direction)
    }

    pub fn split_with_editor(
        &mut self,
        document_id: DocumentId,
        direction: SplitDirection,
    ) -> PaneId {
        self.insert_split(
            Pane::Editor {
                document_id,
                view: EditorView::default(),
            },
            direction,
        )
    }

    fn insert_split(&mut self, pane: Pane, direction: SplitDirection) -> PaneId {
        let pane_id = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        let split_id = SplitId(self.next_split_id);
        self.next_split_id += 1;
        let inserted = self.layout.split(
            self.focused,
            pane_id,
            split_id,
            direction,
            self.initial_split_ratio,
        );
        debug_assert!(inserted);
        let tabs = match &pane {
            Pane::Empty => Vec::new(),
            Pane::Editor { document_id, .. } => vec![*document_id],
        };
        self.panes.insert(pane_id, pane);
        self.tabs.insert(pane_id, tabs);
        self.focused = pane_id;
        pane_id
    }

    pub fn close_focused(&mut self) -> bool {
        if self.panes.len() == 1 {
            return false;
        }
        let target = self.focused;
        let Some(layout) = self.layout.clone().close(target) else {
            return false;
        };
        self.layout = layout;
        self.panes.remove(&target);
        self.tabs.remove(&target);
        self.focused = *self.panes.keys().next().expect("one pane must remain");
        true
    }

    pub fn calculate(
        &self,
        area: Rect,
        minimum_width: u16,
        minimum_height: u16,
        vertical_divider_thickness: u16,
        horizontal_divider_thickness: u16,
    ) -> CalculatedLayout {
        self.layout.calculate(
            area,
            minimum_width,
            minimum_height,
            vertical_divider_thickness,
            horizontal_divider_thickness,
        )
    }

    pub fn focus_at(&mut self, layout: &CalculatedLayout, x: u16, y: u16) -> bool {
        let Some(id) = layout
            .panes
            .iter()
            .find_map(|(id, area)| area.contains(x, y).then_some(*id))
        else {
            return false;
        };
        self.focused = id;
        true
    }

    pub fn move_focus(&mut self, direction: FocusDirection, layout: &CalculatedLayout) -> bool {
        let Some(target) = nearest_pane(self.focused, direction, &layout.panes) else {
            return false;
        };
        self.focused = target;
        true
    }

    pub fn resize_split(&mut self, split_id: SplitId, ratio: f32) -> bool {
        self.layout.set_ratio(split_id, ratio)
    }

    pub fn set_initial_split_ratio(&mut self, ratio: f32) {
        self.initial_split_ratio = ratio;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_split_shares_document_with_independent_view() {
        let mut workspace = Workspace::with_editor(DocumentId(7), 0.5);
        let original = workspace.focused;
        let created = workspace.split_contextual(SplitDirection::Vertical);
        let Pane::Editor { document_id, view } = &workspace.panes[&created] else {
            panic!("expected editor pane");
        };
        assert_eq!(*document_id, DocumentId(7));
        assert_eq!(*view, EditorView::default());
        let Pane::Editor { view, .. } = workspace.panes.get_mut(&original).unwrap() else {
            panic!("expected editor pane");
        };
        view.cursor = 4;
        let Pane::Editor { view, .. } = &workspace.panes[&created] else {
            panic!("expected editor pane");
        };
        assert_eq!(view.cursor, 0);
    }

    #[test]
    fn final_pane_cannot_close() {
        let mut workspace = Workspace::with_editor(DocumentId(1), 0.5);
        assert!(!workspace.close_focused());
    }
}
