# Mobile Zoom Prevention Testing Guide

## Overview
This guide helps you test and validate that the mobile zoom prevention fixes are working correctly across different devices and browsers.

## Quick Test Checklist

### 1. Basic Zoom Prevention Test
- [ ] Open CodeForge on a mobile device
- [ ] Navigate to the chat interface
- [ ] Tap on the message input textarea
- [ ] **Expected**: No zoom should occur, cursor should appear immediately
- [ ] **If zoom occurs**: The fix needs adjustment

### 2. Multiple Input Types Test
Test these input elements in the CodeForge interface:
- [ ] Chat message textarea
- [ ] File search/filter inputs
- [ ] Git commit message input
- [ ] Settings form inputs (password change)
- [ ] Project selection dropdowns

### 3. Browser Compatibility Test
Test on these mobile browsers:
- [ ] Safari (iOS)
- [ ] Chrome (iOS)
- [ ] Chrome (Android)
- [ ] Firefox (Android)
- [ ] Samsung Internet (Android)

### 4. Device Orientation Test
- [ ] Test in portrait mode
- [ ] Test in landscape mode
- [ ] Rotate device while input is focused
- [ ] **Expected**: No zoom in any orientation

## Using the Mobile Test Page

1. Navigate to `/mobile_test.html` in your CodeForge installation
2. Follow the test sections in order
3. Pay special attention to:
   - Section 1: Basic zoom prevention
   - Section 7: Real-world chat simulation
   - Section 8: Zoom detection monitoring

## Troubleshooting Common Issues

### Issue: Zoom still occurs on iOS Safari
**Solution**: Check that the viewport meta tag includes `user-scalable=no` and `maximum-scale=1.0`

### Issue: Zoom occurs on specific input types
**Solution**: Ensure all input types have `font-size: 16px` and `-webkit-appearance: none`

### Issue: Double-tap zoom still works
**Solution**: Verify `touch-action: manipulation` is applied to interactive elements

## Advanced Testing

### Zoom Detection Script
Use this JavaScript to detect if zoom is occurring:
```javascript
function detectZoom() {
    const zoomLevel = Math.round((window.outerWidth / window.innerWidth) * 100) / 100;
    console.log('Current zoom level:', zoomLevel);
    return zoomLevel > 1.1; // Allow for small variations
}
```

### Performance Testing
- Test with slow network connections
- Test with many messages in chat
- Monitor for any performance impact from zoom prevention code

## Success Criteria

✅ **Complete Success**: No zoom occurs on any input focus across all tested devices and browsers
⚠️ **Partial Success**: Zoom prevented on most devices but issues on specific browser/device combinations
❌ **Failure**: Zoom still occurs on primary target devices (iPhone Safari, Android Chrome)

## Reporting Issues

When reporting zoom issues, include:
1. Device model and OS version
2. Browser name and version
3. Specific input element that triggered zoom
4. Steps to reproduce
5. Screenshot/video if possible
