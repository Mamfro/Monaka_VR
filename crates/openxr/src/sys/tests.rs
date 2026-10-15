use super::*;
use core::mem::{offset_of, size_of};

// The C headers' sizes and offsets with MSVC x64 (sizeof/offsetof).
#[test]
fn layouts_match_c() {
    assert_eq!(size_of::<Posef>(), 28);
    assert_eq!(size_of::<ApplicationInfo>(), 272);
    assert_eq!(size_of::<InstanceCreateInfo>(), 328);
    assert_eq!(size_of::<SystemProperties>(), 304);
    assert_eq!(size_of::<SwapchainCreateInfo>(), 64);
    assert_eq!(size_of::<EventDataBuffer>(), 4016);
    assert_eq!(size_of::<View>(), 64);
    assert_eq!(size_of::<CompositionLayerProjectionView>(), 96);
    assert_eq!(size_of::<CompositionLayerProjection>(), 48);
    assert_eq!(size_of::<CompositionLayerQuad>(), 112);
    assert_eq!(offset_of!(CompositionLayerQuad, subImage), 40);
    assert_eq!(size_of::<FrameEndInfo>(), 40);
    assert_eq!(size_of::<GraphicsRequirementsD3D11KHR>(), 32);
    assert_eq!(size_of::<ReferenceSpaceCreateInfo>(), 48);
    assert_eq!(size_of::<ViewConfigurationView>(), 40);
    assert_eq!(size_of::<EventDataSessionStateChanged>(), 40);
    assert_eq!(size_of::<FrameState>(), 40);
    assert_eq!(size_of::<ViewLocateInfo>(), 40);
    assert_eq!(size_of::<SwapchainSubImage>(), 32);
    assert_eq!(size_of::<ExtensionProperties>(), 152);
    assert_eq!(size_of::<InstanceProperties>(), 152);
    assert_eq!(size_of::<SwapchainImageD3D11KHR>(), 24);
    assert_eq!(size_of::<GraphicsBindingD3D11KHR>(), 24);
    assert_eq!(size_of::<SessionCreateInfo>(), 32);
    assert_eq!(size_of::<SwapchainImageWaitInfo>(), 24);
    assert_eq!(size_of::<ViewState>(), 24);
    assert_eq!(size_of::<SessionBeginInfo>(), 24);
    assert_eq!(size_of::<ActionSetCreateInfo>(), 216);
    assert_eq!(offset_of!(ActionSetCreateInfo, priority), 208);
    assert_eq!(size_of::<ActionCreateInfo>(), 224);
    assert_eq!(offset_of!(ActionCreateInfo, actionType), 80);
    assert_eq!(offset_of!(ActionCreateInfo, subactionPaths), 88);
    assert_eq!(offset_of!(ActionCreateInfo, localizedActionName), 96);
    assert_eq!(size_of::<ActionSuggestedBinding>(), 16);
    assert_eq!(size_of::<InteractionProfileSuggestedBinding>(), 40);
    assert_eq!(offset_of!(InteractionProfileSuggestedBinding, suggestedBindings), 32);
    assert_eq!(size_of::<SessionActionSetsAttachInfo>(), 32);
    assert_eq!(size_of::<ActionSpaceCreateInfo>(), 64);
    assert_eq!(offset_of!(ActionSpaceCreateInfo, poseInActionSpace), 32);
    assert_eq!(size_of::<ActionsSyncInfo>(), 32);
    assert_eq!(size_of::<SpaceLocation>(), 56);
    assert_eq!(offset_of!(SpaceLocation, pose), 24);
    assert_eq!(size_of::<ActionStateGetInfo>(), 32);
    assert_eq!(size_of::<ActionStateBoolean>(), 40);
    assert_eq!(offset_of!(ActionStateBoolean, currentState), 16);
    assert_eq!(offset_of!(ActionStateBoolean, lastChangeTime), 24);
    assert_eq!(offset_of!(ActionStateBoolean, isActive), 32);
    assert_eq!(size_of::<ActionStateFloat>(), 40);
    assert_eq!(offset_of!(ActionStateFloat, isActive), 32);
    assert_eq!(size_of::<ActionStateVector2f>(), 48);
    assert_eq!(offset_of!(ActionStateVector2f, changedSinceLastSync), 24);
    assert_eq!(offset_of!(ActionStateVector2f, lastChangeTime), 32);
    assert_eq!(offset_of!(ActionStateVector2f, isActive), 40);
    assert_eq!(size_of::<ActionStatePose>(), 24);
    assert_eq!(API_VERSION_1_0, (1 << 48) | 63);
    // XR_EXT_hand_tracking (openxr.h, 64-bit).
    assert_eq!(size_of::<SystemHandTrackingPropertiesEXT>(), 24);
    assert_eq!(size_of::<HandTrackerCreateInfoEXT>(), 24);
    assert_eq!(size_of::<HandJointsMotionRangeInfoEXT>(), 24);
    assert_eq!(size_of::<HandJointsLocateInfoEXT>(), 32);
    assert_eq!(size_of::<HandJointLocationEXT>(), 40);
    assert_eq!(offset_of!(HandJointLocationEXT, pose), 8);
    assert_eq!(size_of::<HandJointLocationsEXT>(), 32);
    assert_eq!(offset_of!(HandJointLocationsEXT, jointLocations), 24);
    // XR_KHR_composition_layer_depth.
    assert_eq!(size_of::<CompositionLayerDepthInfoKHR>(), 64);
    assert_eq!(offset_of!(CompositionLayerDepthInfoKHR, subImage), 16);
    assert_eq!(offset_of!(CompositionLayerDepthInfoKHR, minDepth), 48);
    assert_eq!(offset_of!(CompositionLayerDepthInfoKHR, farZ), 60);
    // XR_EXT_eye_gaze_interaction.
    assert_eq!(size_of::<SystemEyeGazeInteractionPropertiesEXT>(), 24);
    assert_eq!(size_of::<EyeGazeSampleTimeEXT>(), 24);
    assert_eq!(offset_of!(EyeGazeSampleTimeEXT, time), 16);
    // XR_FB_eye_tracking_social.
    assert_eq!(size_of::<SystemEyeTrackingPropertiesFB>(), 24);
    assert_eq!(size_of::<EyeTrackerCreateInfoFB>(), 16);
    assert_eq!(size_of::<EyeGazesInfoFB>(), 32);
    assert_eq!(size_of::<EyeGazeFB>(), 36);
    assert_eq!(offset_of!(EyeGazeFB, gazeConfidence), 32);
    assert_eq!(size_of::<EyeGazesFB>(), 96);
    assert_eq!(offset_of!(EyeGazesFB, gaze), 16);
    assert_eq!(offset_of!(EyeGazesFB, time), 88);
}
