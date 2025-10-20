#include <torch/torch.h>
#include <iostream>

int main() {
    std::cout << "LibTorch Version: " << TORCH_VERSION << std::endl;
    std::cout << "CUDA Available: " << torch::cuda::is_available() << std::endl;
    std::cout << "CUDA Device Count: " << torch::cuda::device_count() << std::endl;

    if (torch::cuda::is_available()) {
        torch::Tensor tensor = torch::rand({2, 3}).cuda();
        std::cout << "Created CUDA tensor successfully!" << std::endl;
        std::cout << tensor << std::endl;
    }

    return 0;
}
