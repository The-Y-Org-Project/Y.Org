use y_gpu::GpuContext;

fn main() -> y_gpu::Result<()> {
    let gpu = GpuContext::new()?;
    let gpus = gpu.list_gpus()?;

    println!("Y: {} GPU(s) encontrada(s)", gpus.len());
    for (i, g) in gpus.iter().enumerate() {
        let (a, b, c) = g.vulkan_version;
        println!("  [{}] {} ({:?}) - Vulkan {}.{}.{}", i, g.name, g.device_type, a, b, c);
    }
    Ok(())
}
