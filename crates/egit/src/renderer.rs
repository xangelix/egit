//! What draws egit's window, in words for Preferences: the graphics interface wgpu chose (here
//! Vulkan, Metal, DirectX 12, or OpenGL; in a browser WebGPU where it offers it, else WebGL2)
//! and the GPU it names.

use eframe::wgpu::Backend;

/// Where the words are kept, in the context's data.
fn id() -> egui::Id {
    egui::Id::new("drawn-with")
}

/// Notes what draws the window `creation` made, for [`drawn_with`].
pub(crate) fn note(creation: &eframe::CreationContext<'_>) {
    let Some(state) = &creation.wgpu_render_state else {
        return;
    };
    let info = state.adapter.get_info();
    let words = words(info.backend, info.name.trim());
    creation
        .egui_ctx
        .data_mut(|data| data.insert_temp(id(), words));
}

/// What draws the window, once noted.
pub(crate) fn drawn_with(ctx: &egui::Context) -> Option<String> {
    ctx.data(|data| data.get_temp::<String>(id()))
}

/// `backend` in people's words, and the GPU `name`d (a browser's often names none).
fn words(backend: Backend, name: &str) -> String {
    let interface = match backend {
        Backend::BrowserWebGpu => "WebGPU",
        // A browser's GL is WebGL2.
        Backend::Gl if cfg!(target_arch = "wasm32") => "WebGL2",
        Backend::Gl => "OpenGL",
        Backend::Vulkan => "Vulkan",
        Backend::Metal => "Metal",
        Backend::Dx12 => "DirectX 12",
        // A stand-in that draws nothing (a test's).
        Backend::Noop => "nothing",
    };
    if name.is_empty() {
        interface.to_owned()
    } else {
        format!("{interface} ({name})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_interface_leads_and_the_gpu_follows_where_named() {
        assert_eq!(words(Backend::BrowserWebGpu, ""), "WebGPU");
        assert_eq!(
            words(Backend::Vulkan, "AMD Radeon 780M"),
            "Vulkan (AMD Radeon 780M)"
        );
    }
}
