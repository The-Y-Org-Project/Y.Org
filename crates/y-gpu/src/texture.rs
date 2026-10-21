use crate::{Result, find_memory_type};
use ash::vk;

pub struct Texture {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    sampler: vk::Sampler,
    pool: vk::DescriptorPool,
    pub set: vk::DescriptorSet,
    pub width: u32,
    pub height: u32,
}

pub fn create_set_layout(device: &ash::Device) -> Result<vk::DescriptorSetLayout> {
    let bindings = [vk::DescriptorSetLayoutBinding::default()
        .binding(0)
        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
    let info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
    Ok(unsafe { device.create_descriptor_set_layout(&info, None)? })
}

fn check_pixels(width: u32, height: u32, pixels: &[u8]) -> Result<()> {
    if width == 0 || height == 0 || pixels.len() != (width * height * 4) as usize {
        return Err("texture pixel data must be width * height * 4 bytes (RGBA8)".into());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn upload_pixels(
    device: &ash::Device,
    mem_props: &vk::PhysicalDeviceMemoryProperties,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    image: vk::Image,
    width: u32,
    height: u32,
    pixels: &[u8],
    old_layout: vk::ImageLayout,
    src_access: vk::AccessFlags,
    src_stage: vk::PipelineStageFlags,
) -> Result<()> {
    let staging_info = vk::BufferCreateInfo::default()
        .size(pixels.len() as u64)
        .usage(vk::BufferUsageFlags::TRANSFER_SRC)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let staging = unsafe { device.create_buffer(&staging_info, None)? };
    let staging_req = unsafe { device.get_buffer_memory_requirements(staging) };
    let staging_type = find_memory_type(
        mem_props,
        staging_req.memory_type_bits,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    )?;
    let staging_alloc = vk::MemoryAllocateInfo::default()
        .allocation_size(staging_req.size)
        .memory_type_index(staging_type);
    let staging_memory = unsafe { device.allocate_memory(&staging_alloc, None)? };
    unsafe {
        device.bind_buffer_memory(staging, staging_memory, 0)?;
        let ptr = device.map_memory(
            staging_memory,
            0,
            vk::WHOLE_SIZE,
            vk::MemoryMapFlags::empty(),
        )? as *mut u8;
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), ptr, pixels.len());
        device.unmap_memory(staging_memory);
    }

    let range = vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    };

    let cb_alloc = vk::CommandBufferAllocateInfo::default()
        .command_pool(command_pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    let cb = unsafe { device.allocate_command_buffers(&cb_alloc)?[0] };

    unsafe {
        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        device.begin_command_buffer(cb, &begin)?;

        let to_transfer = vk::ImageMemoryBarrier::default()
            .old_layout(old_layout)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_access_mask(src_access)
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(range);
        device.cmd_pipeline_barrier(
            cb,
            src_stage,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_transfer],
        );

        let region = vk::BufferImageCopy::default()
            .image_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .image_extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            });
        device.cmd_copy_buffer_to_image(
            cb,
            staging,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &[region],
        );

        let to_shader = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(range);
        device.cmd_pipeline_barrier(
            cb,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_shader],
        );

        device.end_command_buffer(cb)?;

        let cbs = [cb];
        let submit = vk::SubmitInfo::default().command_buffers(&cbs);
        device.queue_submit(queue, &[submit], vk::Fence::null())?;
        device.queue_wait_idle(queue)?;

        device.free_command_buffers(command_pool, &cbs);
        device.destroy_buffer(staging, None);
        device.free_memory(staging_memory, None);
    }

    Ok(())
}

impl Texture {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &ash::Device,
        mem_props: &vk::PhysicalDeviceMemoryProperties,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        set_layout: vk::DescriptorSetLayout,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<Self> {
        check_pixels(width, height, pixels)?;

        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::B8G8R8A8_UNORM)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = unsafe { device.create_image(&image_info, None)? };
        let image_req = unsafe { device.get_image_memory_requirements(image) };
        let image_type = find_memory_type(
            mem_props,
            image_req.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )?;
        let image_alloc = vk::MemoryAllocateInfo::default()
            .allocation_size(image_req.size)
            .memory_type_index(image_type);
        let memory = unsafe { device.allocate_memory(&image_alloc, None)? };
        unsafe { device.bind_image_memory(image, memory, 0)? };

        upload_pixels(
            device,
            mem_props,
            queue,
            command_pool,
            image,
            width,
            height,
            pixels,
            vk::ImageLayout::UNDEFINED,
            vk::AccessFlags::empty(),
            vk::PipelineStageFlags::TOP_OF_PIPE,
        )?;

        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk::Format::B8G8R8A8_UNORM)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let view = unsafe { device.create_image_view(&view_info, None)? };

        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::NEAREST)
            .min_filter(vk::Filter::NEAREST)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::REPEAT)
            .address_mode_v(vk::SamplerAddressMode::REPEAT)
            .address_mode_w(vk::SamplerAddressMode::REPEAT);
        let sampler = unsafe { device.create_sampler(&sampler_info, None)? };

        let pool_sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(1)
            .pool_sizes(&pool_sizes);
        let pool = unsafe { device.create_descriptor_pool(&pool_info, None)? };

        let layouts = [set_layout];
        let set_alloc = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(&layouts);
        let set = unsafe { device.allocate_descriptor_sets(&set_alloc)?[0] };

        let image_infos = [vk::DescriptorImageInfo::default()
            .sampler(sampler)
            .image_view(view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&image_infos);
        unsafe { device.update_descriptor_sets(&[write], &[]) };

        Ok(Self {
            image,
            memory,
            view,
            sampler,
            pool,
            set,
            width,
            height,
        })
    }

    pub fn update(
        &self,
        device: &ash::Device,
        mem_props: &vk::PhysicalDeviceMemoryProperties,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        pixels: &[u8],
    ) -> Result<()> {
        check_pixels(self.width, self.height, pixels)?;
        upload_pixels(
            device,
            mem_props,
            queue,
            command_pool,
            self.image,
            self.width,
            self.height,
            pixels,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            vk::AccessFlags::SHADER_READ,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
        )
    }

    pub fn destroy(&self, device: &ash::Device) {
        unsafe {
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
            device.free_memory(self.memory, None);
        }
    }
}
