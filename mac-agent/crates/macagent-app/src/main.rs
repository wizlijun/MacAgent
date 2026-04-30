//! macagent entry point.
//!
//! M1.6: eframe普通窗口 + 配对状态机（NotPaired / Pairing / Paired）。
//! 菜单栏图标推迟到后续里程碑。

mod keychain;
mod pair_qr;
mod rtc_glue;
mod ui;

use anyhow::Result;

fn main() -> Result<()> {
    // Build a Tokio runtime for reqwest async tasks.
    let rt = tokio::runtime::Runtime::new()?;
    let handle = rt.handle().clone();

    // Keep the runtime alive for the duration of the eframe event loop.
    // eframe::run_native takes ownership of the App, so we move `rt` into a
    // wrapper that drops it after the event loop exits.
    let app = ui::MacAgentApp::new(handle)?;

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("macagent")
            .with_inner_size([400.0, 300.0])
            .with_resizable(false),
        ..Default::default()
    };

    // eframe::run_native only returns on error (it calls std::process::exit on normal quit).
    eframe::run_native(
        "macagent",
        native_options,
        Box::new(move |_cc| {
            // Wrap in a holder that keeps the Tokio runtime alive.
            Ok(Box::new(RuntimeHolder { rt, app }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {}", e))
}

/// Wraps the app + tokio runtime so the runtime stays alive until the window closes.
struct RuntimeHolder {
    // Must remain alive so spawned tasks can continue running.
    #[allow(dead_code)]
    rt: tokio::runtime::Runtime,
    app: ui::MacAgentApp,
}

impl eframe::App for RuntimeHolder {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.app.update(ctx, frame);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Shutdown the runtime cleanly when the window closes.
        // `rt` is dropped when RuntimeHolder is dropped; this is implicit.
    }
}
