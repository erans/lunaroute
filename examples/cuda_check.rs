// Simple test to check libtorch and CUDA initialization

use std::env;

fn main() {
    println!("=== LibTorch Environment Check ===\n");

    // Check environment variables
    println!("LIBTORCH: {:?}", env::var("LIBTORCH"));
    println!("LD_LIBRARY_PATH: {:?}", env::var("LD_LIBRARY_PATH"));
    println!("CUDA_VISIBLE_DEVICES: {:?}", env::var("CUDA_VISIBLE_DEVICES"));

    println!("\n=== LibTorch CUDA Check ===\n");

    // This will show if libtorch can see CUDA
    println!("Checking CUDA availability...");
    let cuda_available = tch::Cuda::is_available();
    println!("CUDA is_available(): {}", cuda_available);

    let cuda_count = tch::Cuda::device_count();
    println!("CUDA device_count(): {}", cuda_count);

    let cudnn_available = tch::Cuda::cudnn_is_available();
    println!("cuDNN is_available(): {}", cudnn_available);

    println!("\n=== LibTorch Built With ===\n");
    println!("Has CUDA: {}", tch::utils::has_cuda());
    println!("Has MKL: {}", tch::utils::has_mkl());
    println!("Has MPS: {}", tch::utils::has_mps());

    // Try to create a simple CUDA tensor
    if cuda_available {
        println!("\n=== Creating CUDA Tensor ===\n");
        match std::panic::catch_unwind(|| {
            let device = tch::Device::Cuda(0);
            let tensor = tch::Tensor::ones([2, 2], (tch::Kind::Float, device));
            println!("Successfully created CUDA tensor!");
            println!("{:?}", tensor);
        }) {
            Ok(_) => println!("✅ CUDA tensor creation successful"),
            Err(e) => println!("❌ CUDA tensor creation failed: {:?}", e),
        }
    }
}
