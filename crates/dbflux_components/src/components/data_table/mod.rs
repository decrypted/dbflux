mod annotation;
pub mod clipboard;
pub mod column_layout;
pub mod document;
mod events;
pub mod model;
mod record;
pub mod selection;
pub mod source;
mod state;
mod table;
#[cfg(test)]
mod tests;
mod theme;
mod totals;

pub use annotation::HeaderAnnotation;
pub use column_layout::{ColumnLayout, ColumnLayoutSpec};
pub use document::{ColumnGroupHeader, DocumentColumnHeader, DocumentPresentation, GroupSpan};
pub use events::{ContextMenuAction, DataTableEvent, Direction, Edge, FilterOperator, SortState};
pub use model::TableModel;
pub use state::{
    DataTableState, EditorSuggestion, GeneratedIdOffer, HeaderClickSetting, ModelSwap,
    header_click_sorts, set_header_click_sorts,
};
pub use table::{CONTEXT, DataTable, actions, context_menu_keystroke};
pub use theme::ROW_NUMBER_WIDTH;
<<<<<<< bb09478a
pub use totals::{NumericTotals, SelectionTotals};
=======
// The row list and cell painter moved to `bolt::components::grid`;
// re-exported here so the old surface keeps compiling.
pub use bolt::components::grid::{CellPaint, GridCellCoord, GridHost, cell_paint, render_rows};
>>>>>>> 08605376
