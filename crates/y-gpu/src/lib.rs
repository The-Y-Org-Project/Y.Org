use ash::{Entry, Instance, khr, vk};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub struct GpuInfo {
    pub name: String,
    pub device_type: vk::PhysicalDeviceType,
    pub vulkan_version: (u32, u32, u32),
}

pub struct GpuContext {
    entry: Entry,
    instance: Instance,
}

pub struct Surface {
    loader: khr::surface::Instance,
    raw: vk::SurfaceKHR,
}

pub struct Renderer {
    device: ash::Device,
    swapchain_loader: khr::swapchain::Device,
    swapchain: vk::SwapchainKHR,
    pub images: Vec<vk::Image>,
    pub format: vk::Format,
    pub extent: vk::Extent2D,
    pub queue: vk::Queue,
    pub queue_family: u32,
    pub gpu_name: String,
}

impl GpuContext {
    pub fn new(display: RawDisplayHandle) -> Result<Self> {
        let entry = unsafe { Entry::load()? };

        let app_info = vk::ApplicationInfo::default()
            .application_name(c"Y Window System")
            .application_version(vk::make_api_version(0, 0, 1, 0))
            .engine_name(c"Y")
            .api_version(vk::API_VERSION_1_1);

        let extensions = ash_window::enumerate_required_extensions(display)?;

        let create_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_extension_names(extensions);

        let instance = unsafe { entry.create_instance(&create_info, None)? };

        Ok(Self { entry, instance })
    }

    pub fn list_gpus(&self) -> Result<Vec<GpuInfo>> {
        let devices = unsafe { self.instance.enumerate_physical_devices()? };
        let mut out = Vec::new();

        for dev in devices {
            let p = unsafe { self.instance.get_physical_device_properties(dev) };
            out.push(GpuInfo {
                name: p.device_name_as_c_str()?.to_string_lossy().into_owned(),
                device_type: p.device_type,
                vulkan_version: (
                    vk::api_version_major(p.api_version),
                    vk::api_version_minor(p.api_version),
                    vk::api_version_patch(p.api_version),
                ),
            });
        }
        Ok(out)
    }

    pub fn create_surface(
        &self,
        display: RawDisplayHandle,
        window: RawWindowHandle,
    ) -> Result<Surface> {
        let raw = unsafe {
            ash_window::create_surface(&self.entry, &self.instance, display, window, None)?
        };
        let loader = khr::surface::Instance::new(&self.entry, &self.instance);
        Ok(Surface { loader, raw })
    }

    pub fn create_renderer(
        &self,
        surface: &Surface,
        size: (u32, u32),
        gpu_index: usize,
    ) -> Result<Renderer> {
        let devices = unsafe { self.instance.enumerate_physical_devices()? };
        let phys = *devices
            .get(gpu_index)
            .ok_or("índice de GPU inválido (use Y_GPU=0, 1 ou 2)")?;

        let props = unsafe { self.instance.get_physical_device_properties(phys) };
        let gpu_name = props.device_name_as_c_str()?.to_string_lossy().into_owned();

        let families = unsafe {
            self.instance
                .get_physical_device_queue_family_properties(phys)
        };
        let mut queue_family = None;
        for (i, f) in families.iter().enumerate() {
            let can_present = unsafe {
                surface
                    .loader
                    .get_physical_device_surface_support(phys, i as u32, surface.raw)?
            };
            if f.queue_flags.contains(vk::QueueFlags::GRAPHICS) && can_present {
                queue_family = Some(i as u32);
                break;
            }
        }
        let queue_family = queue_family.ok_or("nenhuma fila gráfica com suporte a apresentação")?;

        let priorities = [1.0f32];
        let queue_infos = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family)
            .queue_priorities(&priorities)];
        let device_exts = [khr::swapchain::NAME.as_ptr()];
        let device_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_infos)
            .enabled_extension_names(&device_exts);
        let device = unsafe { self.instance.create_device(phys, &device_info, None)? };
        let queue = unsafe { device.get_device_queue(queue_family, 0) };

        let swapchain_loader = khr::swapchain::Device::new(&self.instance, &device);
        let caps = unsafe {
            surface
                .loader
                .get_physical_device_surface_capabilities(phys, surface.raw)?
        };
        let formats = unsafe {
            surface
                .loader
                .get_physical_device_surface_formats(phys, surface.raw)?
        };
        let format = formats
            .iter()
            .copied()
            .find(|f| {
                f.format == vk::Format::B8G8R8A8_UNORM
                    && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
            })
            .unwrap_or(formats[0]);

        let extent = if caps.current_extent.width != u32::MAX {
            caps.current_extent
        } else {
            vk::Extent2D {
                width: size
                    .0
                    .clamp(caps.min_image_extent.width, caps.max_image_extent.width),
                height: size
                    .1
                    .clamp(caps.min_image_extent.height, caps.max_image_extent.height),
            }
        };

        let mut image_count = caps.min_image_count + 1;
        if caps.max_image_count > 0 && image_count > caps.max_image_count {
            image_count = caps.max_image_count;
        }

        let swap_info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface.raw)
            .min_image_count(image_count)
            .image_format(format.format)
            .image_color_space(format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_DST)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(caps.current_transform)
            .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(vk::PresentModeKHR::FIFO)
            .clipped(true);

        let swapchain = unsafe { swapchain_loader.create_swapchain(&swap_info, None)? };
        let images = unsafe { swapchain_loader.get_swapchain_images(swapchain)? };

        Ok(Renderer {
            device,
            swapchain_loader,
            swapchain,
            images,
            format: format.format,
            extent,
            queue,
            queue_family,
            gpu_name,
        })
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            self.swapchain_loader
                .destroy_swapchain(self.swapchain, None);
            self.device.destroy_device(None);
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe { self.loader.destroy_surface(self.raw, None) };
    }
}

impl Drop for GpuContext {
    fn drop(&mut self) {
        unsafe { self.instance.destroy_instance(None) };
    }
}
