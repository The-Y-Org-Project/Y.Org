mod texture;

use ash::{Entry, Instance, khr, vk};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use std::mem::offset_of;
use texture::{PendingUpload, Texture};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const MAX_VERTICES: usize = 16384;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Vertex {
    pub pos: [f32; 2],
    pub color: [f32; 3],
    pub uv: [f32; 2],
}

impl Vertex {
    pub const fn new(pos: [f32; 2], color: [f32; 3]) -> Self {
        Self {
            pos,
            color,
            uv: [0.0, 0.0],
        }
    }

    pub const fn textured(pos: [f32; 2], color: [f32; 3], uv: [f32; 2]) -> Self {
        Self { pos, color, uv }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureId(usize);

pub const SOLID_TEXTURE: TextureId = TextureId(0);

#[derive(Clone, Copy, Debug)]
pub struct DrawCall {
    pub texture: TextureId,
    pub first_vertex: u32,
    pub vertex_count: u32,
}

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
    mem_props: vk::PhysicalDeviceMemoryProperties,
    swapchain_loader: khr::swapchain::Device,
    swapchain: vk::SwapchainKHR,
    render_pass: vk::RenderPass,
    set_layout: vk::DescriptorSetLayout,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    image_views: Vec<vk::ImageView>,
    framebuffers: Vec<vk::Framebuffer>,
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    image_available: vk::Semaphore,
    render_finished: Vec<vk::Semaphore>,
    in_flight: vk::Fence,
    vertex_buffer: vk::Buffer,
    vertex_memory: vk::DeviceMemory,
    vertex_ptr: *mut u8,
    draws: Vec<DrawCall>,
    textures: Vec<Texture>,
    pending_uploads: Vec<PendingUpload>,
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

struct VertexStorage {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    ptr: *mut u8,
}

fn checker_pixels() -> (u32, u32, Vec<u8>) {
    let size = 64u32;
    let cell = 8u32;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let light = (x / cell + y / cell) % 2 == 0;
            let v = if light { 255u8 } else { 217u8 };
            pixels.extend_from_slice(&[v, v, v, 255]);
        }
    }
    (size, size, pixels)
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

    let present_modes = unsafe {
        surface
            .loader
            .get_physical_device_surface_present_modes(phys, surface.raw)?
    };
    let present_mode = if present_modes.contains(&vk::PresentModeKHR::MAILBOX) {
        vk::PresentModeKHR::MAILBOX
    } else {
        vk::PresentModeKHR::FIFO
    };

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
        .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
        .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
        .pre_transform(caps.current_transform)
        .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
        .present_mode(present_mode)
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

fn create_render_pass(device: &ash::Device, format: vk::Format) -> Result<vk::RenderPass> {
    let attachments = [vk::AttachmentDescription::default()
        .format(format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::PRESENT_SRC_KHR)];

    let color_refs = [vk::AttachmentReference::default()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];

    let subpasses = [vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&color_refs)];

    let dependencies = [vk::SubpassDependency::default()
        .src_subpass(vk::SUBPASS_EXTERNAL)
        .dst_subpass(0)
        .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)];

    let info = vk::RenderPassCreateInfo::default()
        .attachments(&attachments)
        .subpasses(&subpasses)
        .dependencies(&dependencies);

    Ok(unsafe { device.create_render_pass(&info, None)? })
}

fn load_shader(device: &ash::Device, bytes: &[u8]) -> Result<vk::ShaderModule> {
    let code = ash::util::read_spv(&mut std::io::Cursor::new(bytes))?;
    let info = vk::ShaderModuleCreateInfo::default().code(&code);
    Ok(unsafe { device.create_shader_module(&info, None)? })
}

fn create_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    set_layout: vk::DescriptorSetLayout,
) -> Result<(vk::PipelineLayout, vk::Pipeline)> {
    let vert = load_shader(device, include_bytes!("../shaders/basic.vert.spv"))?;
    let frag = load_shader(device, include_bytes!("../shaders/basic.frag.spv"))?;

    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vert)
            .name(c"main"),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(frag)
            .name(c"main"),
    ];

    let bindings = [vk::VertexInputBindingDescription::default()
        .binding(0)
        .stride(size_of::<Vertex>() as u32)
        .input_rate(vk::VertexInputRate::VERTEX)];
    let attributes = [
        vk::VertexInputAttributeDescription::default()
            .location(0)
            .binding(0)
            .format(vk::Format::R32G32_SFLOAT)
            .offset(offset_of!(Vertex, pos) as u32),
        vk::VertexInputAttributeDescription::default()
            .location(1)
            .binding(0)
            .format(vk::Format::R32G32B32_SFLOAT)
            .offset(offset_of!(Vertex, color) as u32),
        vk::VertexInputAttributeDescription::default()
            .location(2)
            .binding(0)
            .format(vk::Format::R32G32_SFLOAT)
            .offset(offset_of!(Vertex, uv) as u32),
    ];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
        .vertex_binding_descriptions(&bindings)
        .vertex_attribute_descriptions(&attributes);

    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::CLOCKWISE)
        .line_width(1.0);
    let multisample = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(vk::ColorComponentFlags::RGBA)
        .blend_enable(true)
        .src_color_blend_factor(vk::BlendFactor::ONE)
        .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
        .color_blend_op(vk::BlendOp::ADD)
        .src_alpha_blend_factor(vk::BlendFactor::ONE)
        .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
        .alpha_blend_op(vk::BlendOp::ADD)];
    let color_blend =
        vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

    let push_ranges = [vk::PushConstantRange::default()
        .stage_flags(vk::ShaderStageFlags::VERTEX)
        .offset(0)
        .size(8)];
    let set_layouts = [set_layout];
    let layout_info = vk::PipelineLayoutCreateInfo::default()
        .set_layouts(&set_layouts)
        .push_constant_ranges(&push_ranges);
    let layout = unsafe { device.create_pipeline_layout(&layout_info, None)? };

    let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterization)
        .multisample_state(&multisample)
        .color_blend_state(&color_blend)
        .dynamic_state(&dynamic)
        .layout(layout)
        .render_pass(render_pass)
        .subpass(0);

    let created = unsafe {
        device.create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info], None)
    };

    unsafe {
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
    }

    let pipeline = created.map_err(|(_, e)| e)?[0];
    Ok((layout, pipeline))
}

fn build_targets(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    images: &[vk::Image],
    format: vk::Format,
    extent: vk::Extent2D,
) -> Result<(Vec<vk::ImageView>, Vec<vk::Framebuffer>)> {
    let mut views = Vec::with_capacity(images.len());
    let mut framebuffers = Vec::with_capacity(images.len());

    for &image in images {
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let view = unsafe { device.create_image_view(&view_info, None)? };
        views.push(view);

        let attachments = [view];
        let fb_info = vk::FramebufferCreateInfo::default()
            .render_pass(render_pass)
            .attachments(&attachments)
            .width(extent.width)
            .height(extent.height)
            .layers(1);
        framebuffers.push(unsafe { device.create_framebuffer(&fb_info, None)? });
    }

    Ok((views, framebuffers))
}

pub(crate) fn find_memory_type(
    props: &vk::PhysicalDeviceMemoryProperties,
    type_bits: u32,
    flags: vk::MemoryPropertyFlags,
) -> Result<u32> {
    for i in 0..props.memory_type_count {
        let allowed = type_bits & (1 << i) != 0;
        let matches = props.memory_types[i as usize]
            .property_flags
            .contains(flags);
        if allowed && matches {
            return Ok(i);
        }
    }
    Err("no suitable memory type found".into())
}

fn create_vertex_storage(
    device: &ash::Device,
    mem_props: &vk::PhysicalDeviceMemoryProperties,
) -> Result<VertexStorage> {
    let size = (MAX_VERTICES * size_of::<Vertex>()) as u64;

    let buffer_info = vk::BufferCreateInfo::default()
        .size(size)
        .usage(vk::BufferUsageFlags::VERTEX_BUFFER)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = unsafe { device.create_buffer(&buffer_info, None)? };

    let req = unsafe { device.get_buffer_memory_requirements(buffer) };
    let memory_type = find_memory_type(
        mem_props,
        req.memory_type_bits,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    )?;

    let alloc_info = vk::MemoryAllocateInfo::default()
        .allocation_size(req.size)
        .memory_type_index(memory_type);
    let memory = unsafe { device.allocate_memory(&alloc_info, None)? };

    unsafe { device.bind_buffer_memory(buffer, memory, 0)? };
    let ptr = unsafe { device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? }
        as *mut u8;

    Ok(VertexStorage {
        buffer,
        memory,
        ptr,
    })
}

impl GpuContext {
    pub fn new(display: RawDisplayHandle) -> Result<Self> {
        let entry = unsafe { Entry::load()? };

        let app_info = vk::ApplicationInfo::default()
            .application_name(c"Y.Org")
            .application_version(vk::make_api_version(0, 0, 1, 0))
            .engine_name(c"Y.Org")
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
        let mem_props = unsafe { self.instance.get_physical_device_memory_properties(phys) };

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

        let render_pass = create_render_pass(&device, parts.format)?;
        let set_layout = texture::create_set_layout(&device)?;
        let (pipeline_layout, pipeline) = create_pipeline(&device, render_pass, set_layout)?;
        let (image_views, framebuffers) = build_targets(
            &device,
            render_pass,
            &parts.images,
            parts.format,
            parts.extent,
        )?;

        let storage = create_vertex_storage(&device, &mem_props)?;

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

        let mut pending_uploads: Vec<PendingUpload> = Vec::new();

        let (tex_w, tex_h, tex_pixels) = checker_pixels();
        let texture = Texture::new(
            &device,
            &mem_props,
            queue,
            command_pool,
            &mut pending_uploads,
            set_layout,
            tex_w,
            tex_h,
            &tex_pixels,
        )?;

        Ok(Renderer {
            device,
            phys,
            mem_props,
            swapchain_loader,
            swapchain: parts.swapchain,
            render_pass,
            set_layout,
            pipeline_layout,
            pipeline,
            image_views,
            framebuffers,
            command_pool,
            command_buffer,
            image_available,
            render_finished,
            in_flight,
            vertex_buffer: storage.buffer,
            vertex_memory: storage.memory,
            vertex_ptr: storage.ptr,
            draws: Vec::new(),
            textures: vec![texture],
            pending_uploads,
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
    pub fn create_texture(&mut self, width: u32, height: u32, pixels: &[u8]) -> Result<TextureId> {
        let texture = Texture::new(
            &self.device,
            &self.mem_props,
            self.queue,
            self.command_pool,
            &mut self.pending_uploads,
            self.set_layout,
            width,
            height,
            pixels,
        )?;
        self.textures.push(texture);
        Ok(TextureId(self.textures.len() - 1))
    }

    pub fn update_texture(
        &mut self,
        id: TextureId,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<()> {
        let same_size = self
            .textures
            .get(id.0)
            .map(|t| t.width == width && t.height == height)
            .ok_or("unknown texture id")?;

        if same_size {
            return self.textures[id.0].update(
                &self.device,
                &self.mem_props,
                self.queue,
                self.command_pool,
                &mut self.pending_uploads,
                pixels,
            );
        }

        unsafe { self.device.device_wait_idle()? };
        let fresh = Texture::new(
            &self.device,
            &self.mem_props,
            self.queue,
            self.command_pool,
            &mut self.pending_uploads,
            self.set_layout,
            width,
            height,
            pixels,
        )?;
        let old = std::mem::replace(&mut self.textures[id.0], fresh);
        old.destroy(&self.device);
        Ok(())
    }

    pub fn set_scene(&mut self, vertices: &[Vertex], draws: &[DrawCall]) -> Result<()> {
        texture::reap_pending(&self.device, self.command_pool, &mut self.pending_uploads);

        if vertices.len() > MAX_VERTICES {
            return Err("too many vertices for the vertex buffer".into());
        }
        for draw in draws {
            let end = (draw.first_vertex + draw.vertex_count) as usize;
            if draw.texture.0 >= self.textures.len() || end > vertices.len() {
                return Err("draw call references a missing texture or vertex range".into());
            }
        }

        unsafe {
            self.device
                .wait_for_fences(&[self.in_flight], true, u64::MAX)?;
            std::ptr::copy_nonoverlapping(
                vertices.as_ptr() as *const u8,
                self.vertex_ptr,
                size_of_val(vertices),
            );
        }

        self.draws.clear();
        self.draws.extend_from_slice(draws);
        Ok(())
    }

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
            for &fb in &self.framebuffers {
                self.device.destroy_framebuffer(fb, None);
            }
            for &view in &self.image_views {
                self.device.destroy_image_view(view, None);
            }
            self.swapchain_loader
                .destroy_swapchain(self.swapchain, None);
            for &s in &self.render_finished {
                self.device.destroy_semaphore(s, None);
            }
        }

        let (image_views, framebuffers) = build_targets(
            &self.device,
            self.render_pass,
            &parts.images,
            parts.format,
            parts.extent,
        )?;

        let sem_info = vk::SemaphoreCreateInfo::default();
        let mut render_finished = Vec::with_capacity(parts.images.len());
        for _ in 0..parts.images.len() {
            render_finished.push(unsafe { self.device.create_semaphore(&sem_info, None)? });
        }

        self.swapchain = parts.swapchain;
        self.images = parts.images;
        self.format = parts.format;
        self.extent = parts.extent;
        self.image_views = image_views;
        self.framebuffers = framebuffers;
        self.render_finished = render_finished;
        Ok(())
    }

    pub fn draw_frame(&self, background: [f32; 4]) -> Result<bool> {
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

            let finished = self.render_finished[index as usize];
            let cb = self.command_buffer;

            self.device
                .reset_command_buffer(cb, vk::CommandBufferResetFlags::empty())?;
            let begin_info = vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            self.device.begin_command_buffer(cb, &begin_info)?;

            let clear_values = [vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: background,
                },
            }];
            let pass_info = vk::RenderPassBeginInfo::default()
                .render_pass(self.render_pass)
                .framebuffer(self.framebuffers[index as usize])
                .render_area(vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: self.extent,
                })
                .clear_values(&clear_values);
            self.device
                .cmd_begin_render_pass(cb, &pass_info, vk::SubpassContents::INLINE);

            self.device
                .cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.pipeline);

            let viewport = vk::Viewport {
                x: 0.0,
                y: 0.0,
                width: self.extent.width as f32,
                height: self.extent.height as f32,
                min_depth: 0.0,
                max_depth: 1.0,
            };
            let scissor = vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.extent,
            };
            self.device.cmd_set_viewport(cb, 0, &[viewport]);
            self.device.cmd_set_scissor(cb, 0, &[scissor]);

            let mut resolution = [0u8; 8];
            resolution[..4].copy_from_slice(&(self.extent.width as f32).to_ne_bytes());
            resolution[4..].copy_from_slice(&(self.extent.height as f32).to_ne_bytes());
            self.device.cmd_push_constants(
                cb,
                self.pipeline_layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                &resolution,
            );

            self.device
                .cmd_bind_vertex_buffers(cb, 0, &[self.vertex_buffer], &[0]);
            for draw in &self.draws {
                self.device.cmd_bind_descriptor_sets(
                    cb,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.pipeline_layout,
                    0,
                    &[self.textures[draw.texture.0].set],
                    &[],
                );
                self.device
                    .cmd_draw(cb, draw.vertex_count, 1, draw.first_vertex, 0);
            }

            self.device.cmd_end_render_pass(cb);
            self.device.end_command_buffer(cb)?;

            let wait_semaphores = [self.image_available];
            let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
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
            for p in self.pending_uploads.drain(..) {
                self.device.destroy_fence(p.fence, None);
                self.device.destroy_buffer(p.staging, None);
                self.device.free_memory(p.staging_memory, None);
                self.device.free_command_buffers(self.command_pool, &[p.command_buffer]);
            }
            for texture in &self.textures {
                texture.destroy(&self.device);
            }
            self.device.unmap_memory(self.vertex_memory);
            self.device.destroy_buffer(self.vertex_buffer, None);
            self.device.free_memory(self.vertex_memory, None);
            self.device.destroy_fence(self.in_flight, None);
            for &s in &self.render_finished {
                self.device.destroy_semaphore(s, None);
            }
            self.device.destroy_semaphore(self.image_available, None);
            self.device.destroy_command_pool(self.command_pool, None);
            for &fb in &self.framebuffers {
                self.device.destroy_framebuffer(fb, None);
            }
            for &view in &self.image_views {
                self.device.destroy_image_view(view, None);
            }
            self.device.destroy_pipeline(self.pipeline, None);
            self.device
                .destroy_pipeline_layout(self.pipeline_layout, None);
            self.device
                .destroy_descriptor_set_layout(self.set_layout, None);
            self.device.destroy_render_pass(self.render_pass, None);
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
