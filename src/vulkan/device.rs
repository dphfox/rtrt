use std::{ffi::{CStr, c_char, CString}, rc::Rc};
use anyhow::Result;
use ash::{vk::{self, PhysicalDevice, Queue, PhysicalDeviceVulkanMemoryModelFeatures, SurfaceCapabilitiesKHR, SurfaceFormatKHR, PresentModeKHR}, Device, extensions::khr::Swapchain};

use super::surface::SurfaceCtx;

fn get_required_device_extensions() -> Vec<CString> {
    vec![
        Swapchain::name().to_owned()
    ]
}

fn current_swapchain_support_impl(
    surface_ctx: &SurfaceCtx,
    physical_device: PhysicalDevice
) -> Result<SwapchainSupportDetails> {
    let capabilities = unsafe { surface_ctx.surface.get_physical_device_surface_capabilities(physical_device, surface_ctx.surface_khr)? };
    let formats = unsafe { surface_ctx.surface.get_physical_device_surface_formats(physical_device, surface_ctx.surface_khr)? };
    let present_modes = unsafe { surface_ctx.surface.get_physical_device_surface_present_modes(physical_device, surface_ctx.surface_khr)? };
    Ok(SwapchainSupportDetails {
        capabilities,
        formats,
        present_modes,
    })
}

fn select_physical_device(
    surface_ctx: &SurfaceCtx,
    required_device_extensions: &[&CStr]
) -> Result<PhysicalDeviceInfo> {
    let devices = unsafe { surface_ctx.instance_ctx.instance.enumerate_physical_devices() }?;
    let devices_and_queues = devices.into_iter()
        .map(|device| Ok((device, find_queue_families(surface_ctx, device)?)))
        .collect::<Result<Vec<_>>>()?;
    devices_and_queues.into_iter()
    .filter_map(|(device, queues)| {
        let (graphics_family_index, present_family_index) = queues?;
        let supports_required_extensions = test_required_extensions(surface_ctx, device, required_device_extensions).ok()?;
        if !supports_required_extensions { return None; }
        let swapchain_support_details = current_swapchain_support_impl(surface_ctx, device).ok()?;
        let swapchain_is_adequate = !swapchain_support_details.formats.is_empty() && !swapchain_support_details.present_modes.is_empty();
        if !swapchain_is_adequate { return None; }
        let props = unsafe { surface_ctx.instance_ctx.instance.get_physical_device_properties(device) };
        let debug_device_name = unsafe { CStr::from_ptr(props.device_name.as_ptr()) }.to_owned();
        let dedup_family_indices = if graphics_family_index == present_family_index { vec![graphics_family_index] } else { vec![graphics_family_index, present_family_index] };
        Some(PhysicalDeviceInfo {
            device,
            graphics_family_index,
            present_family_index,
            dedup_family_indices,
            debug_device_name,
        })
    })
    .next().ok_or(anyhow::anyhow!("No suitable physical device"))
}

fn test_required_extensions(
    surface_ctx: &SurfaceCtx,
    device: PhysicalDevice,
    required_device_extensions: &[&CStr]
) -> Result<bool> {
    let extension_props = unsafe { surface_ctx.instance_ctx.instance.enumerate_device_extension_properties(device)? };
    let extension_names = extension_props.iter()
        .map(|x| unsafe { CStr::from_ptr(x.extension_name.as_ptr()) })
        .collect::<Vec<_>>();
    let has_all_extensions = required_device_extensions.iter().all(|x| extension_names.contains(x));
    Ok(has_all_extensions)
}

fn find_queue_families(
    surface_ctx: &SurfaceCtx,
    device: PhysicalDevice
) -> Result<Option<(u32, u32)>> {
    let mut graphics = None;
    let mut present = None;
    let props = unsafe { surface_ctx.instance_ctx.instance.get_physical_device_queue_family_properties(device) };
    for (index, family) in props.iter().filter(|f| f.queue_count > 0).enumerate() {
        let index = index as u32;
        if family.queue_flags.contains(vk::QueueFlags::GRAPHICS) && graphics.is_none() {
            graphics = Some(index);
        }
        let present_support = unsafe { surface_ctx.surface.get_physical_device_surface_support(device, index, surface_ctx.surface_khr)? };
        if present_support && present.is_none() {
            present = Some(index);
        }
        if let Some(graphics) = graphics {
            if let Some(present) = present {
                return Ok(Some((graphics, present)))
            }
        }
    }
    Ok(None)
}

fn create_logical_device(
    surface_ctx: &SurfaceCtx,
    physical_info: &PhysicalDeviceInfo,
    layer_name_pointers: &[*const c_char],
    required_device_extensions: &[&CStr]
) -> Result<LogicalDeviceInfo> {
    let queue_priorities = [1.0f32];
    let queue_create_infos = physical_info.dedup_family_indices.iter()
        .map(|index| vk::DeviceQueueCreateInfo::builder()
            .queue_family_index(*index)
            .queue_priorities(&queue_priorities)
            .build()
        ).collect::<Vec<_>>();
    let device_extensions_ptrs = required_device_extensions.iter().map(|x| x.as_ptr()).collect::<Vec<_>>();
    let device_features = vk::PhysicalDeviceFeatures::builder().build();
    let device_create_info = vk::DeviceCreateInfo::builder()
        .queue_create_infos(&queue_create_infos)
        .enabled_extension_names(&device_extensions_ptrs)
        .enabled_features(&device_features)
        .enabled_layer_names(layer_name_pointers)
        .push_next(&mut 
            PhysicalDeviceVulkanMemoryModelFeatures::builder()
            .vulkan_memory_model(true)
        )
        .build();
    let device = unsafe { surface_ctx.instance_ctx.instance.create_device(physical_info.device, &device_create_info, None)? };
    let graphics_queue = unsafe { device.get_device_queue(physical_info.graphics_family_index, 0) };
    let present_queue = unsafe { device.get_device_queue(physical_info.present_family_index, 0) };
    Ok(LogicalDeviceInfo {
        device,
        graphics_queue,
        present_queue
    })
}
pub struct DeviceCtx {
    pub surface_ctx: Rc<SurfaceCtx>,
    pub physical_info: PhysicalDeviceInfo,
    pub logical_info: LogicalDeviceInfo
}

impl DeviceCtx {
    pub fn new(
        surface_ctx: Rc<SurfaceCtx>
    ) -> Result<Rc<DeviceCtx>> {
        let required_ext = get_required_device_extensions();
        let required_ext_ref = required_ext.iter().map(CString::as_c_str).collect::<Vec<_>>();
        let physical_info = select_physical_device(&surface_ctx, &required_ext_ref)?;
        let logical_info = create_logical_device(&surface_ctx, &physical_info, &surface_ctx.instance_ctx.layer_name_pointers, &required_ext_ref)?;
        
        log::debug!("DeviceCtx created ({})", physical_info.debug_device_name.to_str().unwrap_or("vkw: device is not nameable"));
        Ok(Rc::new(DeviceCtx {
            surface_ctx,
            physical_info,
            logical_info
        }))
    }

    pub fn current_swapchain_support(
        &self
    ) -> Result<SwapchainSupportDetails> {
        current_swapchain_support_impl(&self.surface_ctx, self.physical_info.device)
    }

    pub fn wait_for_idle(
        &self
    ) -> Result<()> {
        unsafe { self.logical_info.device.device_wait_idle()? }
        Ok(())
    }
}

impl Drop for DeviceCtx {
    fn drop(&mut self) {
        unsafe {
            self.logical_info.device.destroy_device(None);
        }
        log::debug!("DeviceCtx dropped");
    }
}

pub struct PhysicalDeviceInfo {
    pub device: PhysicalDevice,
    pub graphics_family_index: u32,
    pub present_family_index: u32,
    pub dedup_family_indices: Vec<u32>,
    pub debug_device_name: CString
}

pub struct LogicalDeviceInfo {
    pub device: Device,
    pub graphics_queue: Queue,
    pub present_queue: Queue
}

pub struct SwapchainSupportDetails {
    pub capabilities: SurfaceCapabilitiesKHR,
    pub formats: Vec<SurfaceFormatKHR>,
    pub present_modes: Vec<PresentModeKHR>
}