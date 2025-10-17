// Example demonstrating LibTorch with CUDA support
// This shows that CUDA is available and working with the installed libtorch

use tch::{Device, Tensor, Kind};

fn main() {
    println!("=== LibTorch CUDA Test ===\n");

    // Check CUDA availability
    let cuda_available = tch::Cuda::is_available();
    println!("CUDA available: {}", cuda_available);

    if cuda_available {
        let cuda_device_count = tch::Cuda::device_count();
        println!("CUDA device count: {}", cuda_device_count);

        println!("\n=== Running CUDA Operations ===\n");

        // Create tensors on CPU
        let cpu_tensor = Tensor::randn([1000, 1000], (Kind::Float, Device::Cpu));
        println!("Created tensor on CPU: {:?}", cpu_tensor.size());

        // Move tensor to CUDA
        let cuda_tensor = cpu_tensor.to_device(Device::Cuda(0));
        println!("Moved tensor to CUDA device 0");

        // Perform matrix multiplication on GPU
        let result = cuda_tensor.matmul(&cuda_tensor.tr());
        println!("Performed matrix multiplication on GPU");
        println!("Result shape: {:?}", result.size());

        // Move result back to CPU
        let cpu_result = result.to_device(Device::Cpu);
        println!("Moved result back to CPU");

        // Get some statistics
        let mean = cpu_result.mean(Kind::Float);
        let max = cpu_result.max();
        let min = cpu_result.min();

        println!("\n=== Result Statistics ===");
        println!("Mean: {:.4}", f64::try_from(mean).unwrap());
        println!("Max: {:.4}", f64::try_from(max).unwrap());
        println!("Min: {:.4}", f64::try_from(min).unwrap());

        println!("\n✅ CUDA operations completed successfully!");
    } else {
        println!("\n⚠️  CUDA is not available. Running CPU-only test.\n");

        // Fallback to CPU operations
        let tensor1 = Tensor::randn([100, 100], (Kind::Float, Device::Cpu));
        let tensor2 = Tensor::randn([100, 100], (Kind::Float, Device::Cpu));

        let result = tensor1.matmul(&tensor2);
        println!("Performed CPU matrix multiplication");
        println!("Result shape: {:?}", result.size());
    }

    println!("\n=== Test Complete ===");
}
