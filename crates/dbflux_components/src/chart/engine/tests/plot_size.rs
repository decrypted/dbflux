//! What a chart does with a canvas too small for a plot, and Y labels that fit.

use super::*;
use gpui::{Context, IntoElement, Window, div};

/// A plot is drawn from the smallest size worth reading, and not below it.
#[test]
fn a_plot_needs_a_real_size() {
    let (tall, wide) = (
        MARGIN_TOP + MARGIN_BOTTOM + MIN_PLOT_HEIGHT,
        MARGIN_LEFT + MARGIN_RIGHT + MIN_PLOT_WIDTH,
    );

    assert!(plot_fits(wide, tall));
    assert!(!plot_fits(wide, tall - 1.0));
    assert!(!plot_fits(wide - 1.0, tall));
}

/// Five ticks on a 20 px plot: only labels a line apart are drawn.
#[test]
fn y_labels_never_touch() {
    let kept: Vec<usize> = y_labels_that_fit(&[20.0, 15.0, 10.0, 5.0, 0.0], 13.0)
        .into_iter()
        .map(|(index, _)| index)
        .collect();
    assert_eq!(kept, [0, 3]);

    assert_eq!(y_labels_that_fit(&[30.0, 15.0, 0.0], 13.0).len(), 3);
    assert!(y_labels_that_fit(&[], 13.0).is_empty());
}

struct SizedChart {
    chart: gpui::Entity<ChartView>,
    size: gpui::Size<Pixels>,
}

impl gpui::Render for SizedChart {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(self.size.width)
            .h(self.size.height)
            .child(self.chart.clone())
    }
}

/// Too small to draw, a chart leaves no plot area behind for the hover to
/// land in; large enough, it has one.
#[gpui::test]
fn a_small_canvas_has_no_plot_area(cx: &mut gpui::TestAppContext) {
    cx.update(crate::theme::init);

    let mut drawn_at = |width: f32, height: f32| {
        let chart = cx.new(|_| {
            ChartView::build(&partially_overlapping_hosts(), simple_spec(0, &[1]))
                .expect("build should succeed")
        });
        let (_host, window) = cx.add_window_view({
            let chart = chart.clone();
            move |_, _| SizedChart {
                chart,
                size: gpui::size(gpui::px(width), gpui::px(height)),
            }
        });
        window.run_until_parked();
        window.update(|_, cx| chart.read(cx).has_plot_area())
    };

    assert!(drawn_at(600.0, 200.0));
    assert!(!drawn_at(600.0, 30.0), "too short");
    assert!(!drawn_at(70.0, 200.0), "too narrow");
}

#[test]
fn the_too_small_note_resolves_in_every_locale() {
    for locale in ["en", "es", "ko", "pt_BR", "zh_Hans"] {
        let note = dbflux_i18n::t!("chart.too_small_to_draw", locale = locale);
        assert_ne!(note, "chart.too_small_to_draw", "{locale}");
    }
}

/// A keyboard point left on a chart that became too small names nothing, and the
/// keys don't move it, until the chart has room again.
#[gpui::test]
fn a_small_canvas_has_no_keyboard_point(cx: &mut gpui::TestAppContext) {
    cx.update(crate::theme::init);

    let mut spec = simple_spec(0, &[1]);
    spec.binding.group_by = Some(2);
    let chart = cx.new(|_| {
        ChartView::build(&partially_overlapping_hosts(), spec).expect("build should succeed")
    });
    chart.update(cx, |chart, cx| assert!(chart.step_keyboard_point(1, cx)));

    let (host, window) = cx.add_window_view({
        let chart = chart.clone();
        move |_, _| SizedChart {
            chart,
            size: gpui::size(gpui::px(600.0), gpui::px(30.0)),
        }
    });
    window.run_until_parked();

    window.update(|_, cx| {
        let chart = chart.read(cx);
        assert!(chart.keyboard_point().is_none());
        assert!(chart.hover_data_x().is_none());
    });
    window.update(|_, cx| {
        chart.update(cx, |chart, cx| {
            assert!(!chart.step_keyboard_point(1, cx));
            assert!(!chart.jump_keyboard_point(true, cx));
            assert!(!chart.step_keyboard_series(1, cx));
        });
    });

    window.update(|_, cx| {
        host.update(cx, |host, cx| {
            host.size = gpui::size(gpui::px(600.0), gpui::px(200.0));
            cx.notify();
        });
    });
    window.run_until_parked();

    window.update(|_, cx| assert!(chart.read(cx).keyboard_point().is_some()));
}
