use ash::{vk, Entry, Instance};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub struct GpuInfo {
    pub name: String,
    pub device_type: vk::PhysicalDeviceType,
    pub vulkan_version: (u32, u32, u32),
}

pub struct GpuContext {
    _entry: Entry,
    instance: Instance,
}

impl GpuContext {
    pub fn new() -> Result<Self> {
        let entry = unsafe { Entry::load()? };

        let app_info = vk::ApplicationInfo::default()
            .application_name(c"Y Window System")
            .application_version(vk::make_api_version(0, 0, 1, 0))
            .engine_name(c"Y")
            .api_version(vk::API_VERSION_1_1);

        let create_info = vk::InstanceCreateInfo::default().application_info(&app_info);
        let instance = unsafe { entry.create_instance(&create_info, None)? };

        Ok(Self { _entry: entry, instance })
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
}

impl Drop for GpuContext {
    fn drop(&mut self) {
        unsafe { self.instance.destroy_instance(None) };
    }
}
