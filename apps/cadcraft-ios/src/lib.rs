//! Native CADCraft iPadOS runner: the Rust static library the Xcode harness in `packaging/ios`
//! links. UIKit startup and the Metal surface lifecycle belong to winit and eframe; this crate adds
//! the single UIKit scene, safe-area insets, document pickers and the browser.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(target_os = "ios")]
mod app;
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
mod apple_entry;
#[cfg(any(target_os = "ios", test))]
mod clipboard;
#[cfg(any(target_os = "ios", test))]
mod documents;
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
mod gc_keyboard;
#[cfg(any(target_os = "ios", test))]
mod hw_keys;
#[cfg(any(target_os = "ios", test))]
mod logging;
#[cfg(any(target_os = "ios", test))]
mod pencil;
#[cfg(target_os = "ios")]
mod safe_area;
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
mod scenes;
#[cfg(target_os = "ios")]
mod services;
#[cfg(any(target_os = "ios", test))]
mod storage;
#[cfg(any(target_os = "ios", test))]
mod workspace;

/// Stencil bits of the window's render pass (as in the desktop app).
const STENCIL_BITS: u8 = 8;

/// Configure Metal explicitly; UIKit/winit supplies the current window dimensions.
pub fn native_options() -> eframe::NativeOptions {
    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends = eframe::wgpu::Backends::METAL;
    setup.device_descriptor = std::sync::Arc::new(|adapter| {
        let available = adapter.limits();
        let requested = eframe::wgpu::Limits { max_texture_dimension_2d: 8192, ..Default::default() };
        eframe::wgpu::DeviceDescriptor {
            label: Some("cadcraft iOS Metal device"),
            required_limits: compatible_limits(requested, &available),
            ..Default::default()
        }
    });
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_app_id("ai.storyteller.cadcraft").with_title("CADCraft"),
        renderer: eframe::Renderer::Wgpu,
        // The canvas clips layout viewports with a stencil buffer, as on the desktop.
        stencil_buffer: STENCIL_BITS,
        run_and_return: false,
        wgpu_options: eframe::WgpuConfiguration { wgpu_setup: eframe::egui_wgpu::WgpuSetup::CreateNew(setup), ..Default::default() },
        ..Default::default()
    }
}

/// egui uses two inter-stage locations; Metal simulators may expose 15 instead of wgpu's default
/// 16. Keep every other limit and the default feature set.
fn compatible_limits(mut requested: eframe::wgpu::Limits, available: &eframe::wgpu::Limits) -> eframe::wgpu::Limits {
    // Below two, device creation must fail rather than pretend the egui shader is supported.
    requested.max_inter_stage_shader_variables = requested.max_inter_stage_shader_variables.min(available.max_inter_stage_shader_variables).max(2);
    requested
}

/// File format hooks for the engine (it stays I/O-agnostic): the same readers and writers the
/// desktop app installs.
#[cfg(target_os = "ios")]
fn install_io() {
    cadcraft_engine::cmd::file::set_io(cadcraft_engine::cmd::file::IoHooks {
        read: |b, name| cadcraft_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadcraft_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadcraft_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
    });
    cadcraft_engine::cmd::file::set_write_as(|d, name, o| {
        cadcraft_io::write_with(d, name, &cadcraft_io::SaveOptions { version: o.version, binary: o.binary }).map_err(|e| e.to_string())
    });
    // AutoLISP's entget, entmake and entmod see entities as DXF groups.
    cadcraft_engine::cmd::file::set_entity_codec(cadcraft_engine::cmd::file::EntityCodec {
        groups: cadcraft_io::entity_groups,
        entity: cadcraft_io::entity_from_groups,
    });
}

/// Launch on iOS's main thread; other platforms return an error without a window.
pub fn run() -> Result<(), String> {
    #[cfg(target_os = "ios")]
    {
        use cadcraft_engine::Session;
        use cadcraft_ui_egui::CadApp;

        if let Err(error) = logging::install() {
            eprintln!("CADCraft logging unavailable: {error}");
        }
        log::info!("Starting CADCraft iPadOS with Metal");
        install_io();
        scenes::register();
        let root = match services::create() {
            Ok(platform) => Some(platform.root),
            Err(error) => {
                log::error!("iOS platform services unavailable: {error}");
                None
            }
        };
        eframe::run_native(
            "CADCraft",
            native_options(),
            Box::new(move |cc| {
                scenes::attach(cc)?;
                gc_keyboard::install(cc.egui_ctx.clone());
                // Apple Pencil hover (the crosshair follows the floating tip) and pressure/tilt/roll.
                if let Err(error) = pencil::install(pencil::bridge::PenFeed::default(), &cc.egui_ctx) {
                    log::error!("Apple Pencil input unavailable: {error}");
                }
                // Hide the status bar through the safe platform API so it cannot overlay egui's menu.
                use winit::platform::ios::WindowExtIOS;
                if let Some(window) = cc.winit_window() {
                    window.set_prefers_status_bar_hidden(true);
                }
                let mut app = CadApp::new(Session::empty(), services::ios(root.clone(), cc.egui_ctx.clone()));
                // The panel layout and which palettes show, kept from the last run.
                let (workspace, restored, workspace_error) = workspace::Workspace::at(root.as_ref().map(|r| r.join("workspace.json")));
                if let Some(ui) = restored {
                    app.ui = ui;
                }
                if let Some(error) = &workspace_error {
                    log::warn!("CADCraft workspace: {error}");
                    app.set_status(error);
                }
                app.system_languages = sys_locale::get_locales().collect();
                if let Some(root) = &root {
                    // Settings kept by an earlier run: the profile (Options), the interface and autosave preferences.
                    let state = services::load_state(root);
                    if let Some(profile) = &state.profile {
                        app.load_profile(profile);
                    }
                    if let Some(prefs) = &state.prefs {
                        app.load_prefs(prefs);
                    }
                    // Recent files that are managed copies in an earlier install of the app container.
                    storage::remap_recent_files(&mut app.ui.recent_files, root);
                    if let Some(prefs) = &state.autosave {
                        cadcraft_ui_egui::autosave::load_prefs(prefs);
                    }
                    app.enable_autosave(&root.join("autosave"));
                    // Startup LISP (acad.lsp, acaddoc.lsp) is read from `<support>/support`.
                    let support = root.join("support");
                    match std::fs::create_dir_all(&support) {
                        Ok(()) => app.session.set_lisp_support_dir(Some(support)),
                        Err(error) => log::warn!("no LISP support folder {}: {error}", support.display()),
                    }
                }
                if let Some(rs) = &cc.wgpu_render_state {
                    app.set_wgpu(rs, eframe::egui_wgpu::depth_format_from_bits(0, STENCIL_BITS));
                }
                if app.session.docs.is_empty() {
                    app.session.new_drawing(false);
                }
                Ok(Box::new(app::IosApp::new(app, root, workspace, workspace_error)))
            }),
        )
        .map_err(|error| format!("CADCraft iOS startup failed: {error}"))
    }
    #[cfg(not(target_os = "ios"))]
    {
        Err("CADCraft iOS runner requires an iOS device or simulator".into())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn simulator_varying_limit_does_not_reduce_other_limits() {
        let requested = eframe::wgpu::Limits { max_texture_dimension_2d: 8192, max_buffer_size: 512 * 1024 * 1024, ..Default::default() };
        let available = eframe::wgpu::Limits { max_inter_stage_shader_variables: 15, ..requested.clone() };
        let actual = super::compatible_limits(requested.clone(), &available);
        assert_eq!(actual, eframe::wgpu::Limits { max_inter_stage_shader_variables: 15, ..requested });
    }

    #[test]
    fn adapters_below_egui_minimum_cannot_silently_lower_shader_requirements() {
        let available = eframe::wgpu::Limits { max_inter_stage_shader_variables: 1, ..Default::default() };
        assert_eq!(super::compatible_limits(eframe::wgpu::Limits::default(), &available).max_inter_stage_shader_variables, 2);
    }

    #[test]
    fn capable_adapters_keep_the_egui_varying_requirement() {
        let requested = eframe::wgpu::Limits::default();
        let available = eframe::wgpu::Limits { max_inter_stage_shader_variables: 32, ..requested.clone() };
        assert_eq!(super::compatible_limits(requested.clone(), &available), requested);
    }

    #[test]
    fn runner_rejects_non_ios_hosts() {
        if !cfg!(target_os = "ios") {
            assert!(super::run().unwrap_err().contains("iOS"));
        }
    }

    #[test]
    fn viewport_uses_os_size_and_only_metal() {
        let options = super::native_options();
        assert_eq!(options.renderer, eframe::Renderer::Wgpu);
        assert_eq!(options.viewport.app_id.as_deref(), Some("ai.storyteller.cadcraft"));
        assert!(options.viewport.inner_size.is_none());
        assert!(!options.run_and_return);
        match options.wgpu_options.wgpu_setup {
            eframe::egui_wgpu::WgpuSetup::CreateNew(setup) => {
                assert_eq!(setup.instance_descriptor.backends, eframe::wgpu::Backends::METAL);
            }
            _ => panic!("runner must create its own Metal device"),
        }
    }
}
