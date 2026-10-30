//! What every document that opens the modal cell editor wires up alike: the
//! toast after its Copy, and the store that keeps the dialog's size across
//! sessions.

use dbflux_components::modals::{
    CellEditorCopiedEvent, EditorGeometry, install_editor_geometry_store,
};
use dbflux_storage::UiStateRepository;
use dbflux_ui_base::AppStateEntity;
use dbflux_ui_base::toast::{Toast, now_hms};
use gpui::{App, Entity};

/// Key under which the cell editor's size is stored in `st_ui_state`.
pub(crate) const CELL_EDITOR_GEOMETRY_KEY: &str =
    dbflux_storage::ui_state_keys::CELL_EDITOR_GEOMETRY;

/// Persists the cell editor's size in `st_ui_state`. Only the first call of
/// the app installs the store; it reads the stored size once.
pub(crate) fn install_geometry_store(app_state: &Entity<AppStateEntity>, cx: &mut App) {
    let storage = app_state.read(cx).storage_runtime();
    let reader = storage.ui_state();
    let writer = storage.ui_state();

    install_editor_geometry_store(
        move || match reader.get(CELL_EDITOR_GEOMETRY_KEY) {
            Ok(Some(json)) => parse_geometry(&json),
            Ok(None) => None,
            Err(error) => {
                log::warn!("Could not read the saved cell editor size: {error}");
                None
            }
        },
        move |geometry| store_geometry(&writer, geometry),
        cx,
    );
}

/// Saves `geometry`, or deletes the saved one after a Reset size.
fn store_geometry(writer: &UiStateRepository, geometry: Option<EditorGeometry>) {
    let Some(geometry) = geometry else {
        if let Err(error) = writer.delete(CELL_EDITOR_GEOMETRY_KEY) {
            log::error!("Could not forget the saved cell editor size: {error}");
        }
        return;
    };

    match serde_json::to_string(&geometry) {
        Ok(json) => {
            if let Err(error) = writer.set(CELL_EDITOR_GEOMETRY_KEY, &json) {
                log::error!("Could not save the cell editor size: {error}");
            }
        }
        Err(error) => log::error!("Could not encode the cell editor size: {error}"),
    }
}

/// The stored size, or `None` for a value another version wrote.
fn parse_geometry(json: &str) -> Option<EditorGeometry> {
    serde_json::from_str(json)
        .map_err(|error| log::warn!("Ignoring the saved cell editor size: {error}"))
        .ok()
}

/// The short toast after the editor's Copy or Copy compact.
pub(crate) fn copied_toast(event: &CellEditorCopiedEvent, cx: &mut App) {
    let message = if event.compact {
        dbflux_i18n::t!("modals.cell_editor.copied_compact")
    } else {
        dbflux_i18n::t!("modals.cell_editor.copied")
    };

    Toast::success(message).meta_right(now_hms()).push(cx);
}

#[cfg(test)]
mod tests {
    use super::parse_geometry;
    use dbflux_components::modals::EditorGeometry;

    #[test]
    fn the_stored_size_round_trips() {
        let geometry = EditorGeometry {
            width: 900.0,
            height: 700.0,
            maximized: true,
        };
        let json = serde_json::to_string(&geometry).expect("encodes");

        assert_eq!(parse_geometry(&json), Some(geometry));
        assert_eq!(parse_geometry("\"bottom\""), None);
    }

    #[test]
    fn the_copy_toasts_resolve_in_every_locale() {
        for key in [
            "modals.cell_editor.copied",
            "modals.cell_editor.copied_compact",
        ] {
            for locale in ["en", "es", "ko", "zh_Hans", "pt_BR"] {
                let text = dbflux_i18n::t!(key, locale = locale);
                assert!(!text.is_empty() && text != key, "{locale} missing {key}");
            }
        }
    }
}
