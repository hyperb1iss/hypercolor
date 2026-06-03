use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::Arc;

use ash::{khr, vk};

use super::{LinuxGlFramebufferImportDescriptor, LinuxGpuInteropError, Result};

pub(super) struct ExportableVulkanImage {
    raw_device: ash::Device,
    image: Option<vk::Image>,
    memory: Option<vk::DeviceMemory>,
    pub(super) memory_fd: OwnedFd,
    pub(super) allocation_size: u64,
}

pub(super) struct ImportedWgpuTexture {
    pub(super) texture: Arc<wgpu::Texture>,
    pub(super) view: Arc<wgpu::TextureView>,
}

impl ExportableVulkanImage {
    pub(super) fn create(
        hal_device: &wgpu_hal::vulkan::Device,
        descriptor: LinuxGlFramebufferImportDescriptor,
    ) -> Result<Self> {
        if !hal_device
            .enabled_device_extensions()
            .contains(&khr::external_memory_fd::NAME)
        {
            return Err(LinuxGpuInteropError::MissingVulkanDeviceExtension(
                "VK_KHR_external_memory_fd",
            ));
        }

        let raw_device = hal_device.raw_device().clone();
        let mut external_memory = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(descriptor.format.vk_format())
            .extent(vk::Extent3D {
                width: descriptor.width,
                height: descriptor.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(
                vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_SRC
                    | vk::ImageUsageFlags::COLOR_ATTACHMENT,
            )
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut external_memory);

        // SAFETY: image_info is fully initialized and uses the active wgpu
        // Vulkan device; allocation ownership is transferred to Self.
        let image = unsafe { raw_device.create_image(&image_info, None) }.map_err(|result| {
            LinuxGpuInteropError::Vulkan {
                operation: "create_image",
                result,
            }
        })?;

        // SAFETY: image was created on raw_device and is valid until cleanup.
        let requirements = unsafe { raw_device.get_image_memory_requirements(image) };
        let memory_type_index = find_memory_type_index(
            hal_device.shared_instance().raw_instance(),
            hal_device.raw_physical_device(),
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )?;

        let mut dedicated_info = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let mut export_info = vk::ExportMemoryAllocateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
        let memory_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type_index)
            .push_next(&mut dedicated_info)
            .push_next(&mut export_info);

        // SAFETY: memory_info requests a dedicated exportable allocation for
        // the image created above on the same device.
        let memory =
            unsafe { raw_device.allocate_memory(&memory_info, None) }.map_err(|result| {
                // SAFETY: image was created on raw_device and has not been bound or
                // handed to wgpu yet.
                unsafe { raw_device.destroy_image(image, None) };
                LinuxGpuInteropError::Vulkan {
                    operation: "allocate_memory",
                    result,
                }
            })?;

        // SAFETY: image and memory were created on the same device, and the
        // allocation satisfies the image memory requirements.
        if let Err(result) = unsafe { raw_device.bind_image_memory(image, memory, 0) } {
            // SAFETY: both resources were created on raw_device and have not
            // been handed to wgpu yet.
            unsafe {
                raw_device.free_memory(memory, None);
                raw_device.destroy_image(image, None);
            }
            return Err(LinuxGpuInteropError::Vulkan {
                operation: "bind_image_memory",
                result,
            });
        }

        let external_memory_fd = khr::external_memory_fd::Device::new(
            hal_device.shared_instance().raw_instance(),
            &raw_device,
        );
        let fd_info = vk::MemoryGetFdInfoKHR::default()
            .memory(memory)
            .handle_type(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
        // SAFETY: memory was allocated with ExportMemoryAllocateInfo for
        // OPAQUE_FD, and the returned descriptor becomes owned by OwnedFd.
        let memory_fd =
            unsafe { external_memory_fd.get_memory_fd(&fd_info) }.map_err(|result| {
                // SAFETY: both resources were created on raw_device and have not
                // been handed to wgpu yet.
                unsafe {
                    raw_device.free_memory(memory, None);
                    raw_device.destroy_image(image, None);
                }
                LinuxGpuInteropError::Vulkan {
                    operation: "get_memory_fd",
                    result,
                }
            })?;
        // SAFETY: Vulkan returned a newly owned POSIX file descriptor.
        let memory_fd = unsafe { OwnedFd::from_raw_fd(memory_fd) };

        Ok(Self {
            raw_device,
            image: Some(image),
            memory: Some(memory),
            memory_fd,
            allocation_size: requirements.size,
        })
    }

    pub(super) fn wrap_as_wgpu_texture(
        &mut self,
        device: &wgpu::Device,
        hal_device: &wgpu_hal::vulkan::Device,
        descriptor: LinuxGlFramebufferImportDescriptor,
    ) -> Result<ImportedWgpuTexture> {
        let image = self.image.take().ok_or(LinuxGpuInteropError::Vulkan {
            operation: "wrap_image",
            result: vk::Result::ERROR_UNKNOWN,
        })?;
        let memory = self.memory.take().ok_or(LinuxGpuInteropError::Vulkan {
            operation: "wrap_memory",
            result: vk::Result::ERROR_UNKNOWN,
        })?;

        let raw_device = self.raw_device.clone();
        let drop_callback: wgpu_hal::DropCallback = Box::new(move || {
            // SAFETY: ownership of image and memory moved into this callback,
            // which wgpu-hal invokes after all GPU uses are complete.
            unsafe {
                raw_device.destroy_image(image, None);
                raw_device.free_memory(memory, None);
            }
        });

        let hal_desc = hal_texture_descriptor(descriptor);
        // SAFETY: image was created from hal_device's raw Vulkan device, bound
        // to exportable memory, and initialized by the completed GL blit.
        let hal_texture = unsafe {
            hal_device.texture_from_raw(
                image,
                &hal_desc,
                Some(drop_callback),
                wgpu_hal::vulkan::TextureMemory::External,
            )
        };
        let wgpu_desc = wgpu_texture_descriptor(descriptor);
        // SAFETY: hal_texture was created from this wgpu device's Vulkan HAL
        // and matches wgpu_desc.
        let texture = unsafe {
            device.create_texture_from_hal::<wgpu_hal::api::Vulkan>(hal_texture, &wgpu_desc)
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        Ok(ImportedWgpuTexture {
            texture: Arc::new(texture),
            view: Arc::new(view),
        })
    }
}

impl Drop for ExportableVulkanImage {
    fn drop(&mut self) {
        if let Some(memory) = self.memory.take() {
            // SAFETY: memory was created on raw_device and was not handed to
            // wgpu because it is still present here.
            unsafe { self.raw_device.free_memory(memory, None) };
        }
        if let Some(image) = self.image.take() {
            // SAFETY: image was created on raw_device and was not handed to
            // wgpu because it is still present here.
            unsafe { self.raw_device.destroy_image(image, None) };
        }
    }
}

fn find_memory_type_index(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    type_bits: u32,
    flags: vk::MemoryPropertyFlags,
) -> Result<u32> {
    // SAFETY: physical_device comes from instance through the active wgpu HAL.
    let memory_properties =
        unsafe { instance.get_physical_device_memory_properties(physical_device) };

    memory_properties
        .memory_types_as_slice()
        .iter()
        .enumerate()
        .find_map(|(index, memory_type)| {
            if index >= u32::BITS as usize {
                return None;
            }
            let type_supported = (type_bits & (1_u32 << index)) != 0;
            let flags_supported = memory_type.property_flags.contains(flags);
            (type_supported && flags_supported).then_some(index as u32)
        })
        .ok_or(LinuxGpuInteropError::MemoryTypeUnavailable)
}

fn wgpu_texture_descriptor(
    descriptor: LinuxGlFramebufferImportDescriptor,
) -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
        label: Some("hypercolor-linux-servo-import"),
        size: wgpu::Extent3d {
            width: descriptor.width,
            height: descriptor.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: descriptor.format.wgpu_format(),
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    }
}

fn hal_texture_descriptor(
    descriptor: LinuxGlFramebufferImportDescriptor,
) -> wgpu_hal::TextureDescriptor<'static> {
    wgpu_hal::TextureDescriptor {
        label: Some("hypercolor-linux-servo-import"),
        size: wgpu::Extent3d {
            width: descriptor.width,
            height: descriptor.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: descriptor.format.wgpu_format(),
        usage: wgpu::TextureUses::RESOURCE
            | wgpu::TextureUses::COPY_SRC
            | wgpu::TextureUses::COLOR_TARGET,
        memory_flags: wgpu_hal::MemoryFlags::empty(),
        view_formats: Vec::new(),
    }
}
