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
    phys: vk::PhysicalDevice,
    swapchain_loader: khr::swapchain::Device,
    swapchain: vk::SwapchainKHR,
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    image_available: vk::Semaphore,
    render_finished: Vec<vk::Semaphore>,
    in_flight: vk::Fence,
    pub images: Vec<vk::Image>,
    pub format: vk::Format,
    pub extent: vk::Extent2D,
    pub queue: vk::Queue,
    pub queue_family: u32,
    pub gpu_name: String,
}

struct SwapchainParts {
    swapchain: vk::SwapchainKHR,
    images: Vec<vk::Image>,
    format: vk::Format,
    extent: vk::Extent2D,
}

fn build_swapchain(
    surface: &Surface,
    swapchain_loader: &khr::swapchain::Device,
    phys: vk::PhysicalDevice,
    size: (u32, u32),
    old: vk::SwapchainKHR,
) -> Result<SwapchainParts> {
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

    let info = vk::SwapchainCreateInfoKHR::default()
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
        .clipped(true)
        .old_swapchain(old);

    let swapchain = unsafe { swapchain_loader.create_swapchain(&info, None)? };
    let images = unsafe { swapchain_loader.get_swapchain_images(swapchain)? };

    Ok(SwapchainParts {
        swapchain,
        images,
        format: format.format,
        extent,
    })
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
            .ok_or("invalid GPU index (use Y_GPU=0, 1 or 2)")?;

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
        let queue_family = queue_family.ok_or("no graphics queue with presentation support")?;

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
        let parts = build_swapchain(
            surface,
            &swapchain_loader,
            phys,
            size,
            vk::SwapchainKHR::null(),
        )?;

        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(queue_family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        let command_pool = unsafe { device.create_command_pool(&pool_info, None)? };

        let alloc_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let command_buffer = unsafe { device.allocate_command_buffers(&alloc_info)?[0] };

        let sem_info = vk::SemaphoreCreateInfo::default();
        let image_available = unsafe { device.create_semaphore(&sem_info, None)? };
        let mut render_finished = Vec::with_capacity(parts.images.len());
        for _ in 0..parts.images.len() {
            render_finished.push(unsafe { device.create_semaphore(&sem_info, None)? });
        }

        let fence_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
        let in_flight = unsafe { device.create_fence(&fence_info, None)? };

        Ok(Renderer {
            device,
            phys,
            swapchain_loader,
            swapchain: parts.swapchain,
            command_pool,
            command_buffer,
            image_available,
            render_finished,
            in_flight,
            images: parts.images,
            format: parts.format,
            extent: parts.extent,
            queue,
            queue_family,
            gpu_name,
        })
    }
}

impl Renderer {
    pub fn resize(&mut self, surface: &Surface, size: (u32, u32)) -> Result<()> {
        if size.0 == 0 || size.1 == 0 {
            return Ok(());
        }

        unsafe { self.device.device_wait_idle()? };

        let parts = build_swapchain(
            surface,
            &self.swapchain_loader,
            self.phys,
            size,
            self.swapchain,
        )?;

        unsafe {
            self.swapchain_loader
                .destroy_swapchain(self.swapchain, None);
            for &s in &self.render_finished {
                self.device.destroy_semaphore(s, None);
            }
        }

        let sem_info = vk::SemaphoreCreateInfo::default();
        let mut render_finished = Vec::with_capacity(parts.images.len());
        for _ in 0..parts.images.len() {
            render_finished.push(unsafe { self.device.create_semaphore(&sem_info, None)? });
        }

        self.swapchain = parts.swapchain;
        self.images = parts.images;
        self.format = parts.format;
        self.extent = parts.extent;
        self.render_finished = render_finished;
        Ok(())
    }

    pub fn draw_clear(&self, color: [f32; 4]) -> Result<bool> {
        let range = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };

        let stale = unsafe {
            self.device
                .wait_for_fences(&[self.in_flight], true, u64::MAX)?;

            let (index, _) = match self.swapchain_loader.acquire_next_image(
                self.swapchain,
                u64::MAX,
                self.image_available,
                vk::Fence::null(),
            ) {
                Ok(v) => v,
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => return Ok(true),
                Err(e) => return Err(e.into()),
            };

            self.device.reset_fences(&[self.in_flight])?;

            let image = self.images[index as usize];
            let finished = self.render_finished[index as usize];
            let cb = self.command_buffer;

            self.device
                .reset_command_buffer(cb, vk::CommandBufferResetFlags::empty())?;
            let begin_info = vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            self.device.begin_command_buffer(cb, &begin_info)?;

            let to_transfer = vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(range);
            self.device.cmd_pipeline_barrier(
                cb,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_transfer],
            );

            let clear = vk::ClearColorValue { float32: color };
            self.device.cmd_clear_color_image(
                cb,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &clear,
                &[range],
            );

            let to_present = vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::empty())
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(range);
            self.device.cmd_pipeline_barrier(
                cb,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_present],
            );

            self.device.end_command_buffer(cb)?;

            let wait_semaphores = [self.image_available];
            let wait_stages = [vk::PipelineStageFlags::TRANSFER];
            let command_buffers = [cb];
            let signal_semaphores = [finished];
            let submit = vk::SubmitInfo::default()
                .wait_semaphores(&wait_semaphores)
                .wait_dst_stage_mask(&wait_stages)
                .command_buffers(&command_buffers)
                .signal_semaphores(&signal_semaphores);
            self.device
                .queue_submit(self.queue, &[submit], self.in_flight)?;

            let swapchains = [self.swapchain];
            let indices = [index];
            let present = vk::PresentInfoKHR::default()
                .wait_semaphores(&signal_semaphores)
                .swapchains(&swapchains)
                .image_indices(&indices);
            match self.swapchain_loader.queue_present(self.queue, &present) {
                Ok(_) => false,
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => true,
                Err(e) => return Err(e.into()),
            }
        };
        Ok(stale)
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_fence(self.in_flight, None);
            for &s in &self.render_finished {
                self.device.destroy_semaphore(s, None);
            }
            self.device.destroy_semaphore(self.image_available, None);
            self.device.destroy_command_pool(self.command_pool, None);
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
