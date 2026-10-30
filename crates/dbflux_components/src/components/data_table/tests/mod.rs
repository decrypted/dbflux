//! Render tests of the grid's column features: fitting a column's width.

mod fit_width;

use std::sync::Arc;

use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, div, px,
};

use super::model::{CellValue, ColumnKind, ColumnSpec, RowData, TableModel};
use super::{DataTable, DataTableState};

/// A window holding one table, 600 by 300 pixels.
struct Harness {
    table: Entity<DataTable>,
}

impl Render for Harness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().w(px(600.0)).h(px(300.0)).child(self.table.clone())
    }
}

fn column(name: &str) -> ColumnSpec {
    ColumnSpec {
        id: name.into(),
        title: name.into(),
        kind: ColumnKind::Text,
        align: gpui::TextAlign::Left,
        type_name: "text".into(),
    }
}

/// A model with the columns `names`, whose rows `cells` builds from a row
/// index and a column index.
fn model(
    names: &[&str],
    rows: usize,
    cells: impl Fn(usize, usize) -> CellValue,
) -> Arc<TableModel> {
    let columns = names.iter().map(|name| column(name)).collect();
    let rows = (0..rows)
        .map(|row| RowData {
            cells: (0..names.len()).map(|col| cells(row, col)).collect(),
        })
        .collect();

    Arc::new(TableModel::new(columns, rows))
}

/// Renders `model` in a [`Harness`] and returns its table state.
fn render_table(
    cx: &mut TestAppContext,
    model: Arc<TableModel>,
) -> (Entity<DataTableState>, &mut VisualTestContext) {
    cx.update(crate::theme::init);

    let holder: std::rc::Rc<std::cell::RefCell<Option<Entity<DataTableState>>>> =
        std::rc::Rc::default();
    let holder_for_view = holder.clone();
    let (_, visual) = cx.add_window_view(move |_window, cx| {
        let state = cx.new(|cx| DataTableState::new(model, cx));
        let table = cx.new(|cx| DataTable::new("columns-table", state.clone(), cx));
        holder_for_view.replace(Some(state));
        Harness { table }
    });
    visual.run_until_parked();

    let state = holder.borrow().clone().expect("the harness built a state");
    (state, visual)
}
