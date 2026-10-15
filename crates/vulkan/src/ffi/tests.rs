use super::*;

#[test]
fn structures_are_laid_out_as_the_headers_say() {
    assert_eq!(size_of::<VkDeviceCreateInfo>(), 72);
    assert_eq!(size_of::<VkSwapchainCreateInfoKHR>(), 104);
    assert_eq!(std::mem::offset_of!(VkSwapchainCreateInfoKHR, imageUsage), 56);
    assert_eq!(size_of::<VkPresentInfoKHR>(), 64);
    assert_eq!(size_of::<VkImageCreateInfo>(), 88);
    assert_eq!(std::mem::offset_of!(VkImageCreateInfo, usage), 56);
    assert_eq!(size_of::<VkImageMemoryBarrier>(), 72);
    assert_eq!(size_of::<VkBufferImageCopy>(), 56);
    assert_eq!(size_of::<VkSubmitInfo>(), 72);
    assert_eq!(size_of::<VkPhysicalDeviceMemoryProperties>(), 520);
    assert_eq!(std::mem::offset_of!(VkPhysicalDeviceIDProperties, deviceLUID), 48);
}
