//! Native Win32 Sprite Window for NEXA Virtual Cursor
//!
//! Creates a lightweight layered topmost window (160x128) with per-pixel alpha,
//! excluded from screen capture (WDA_EXCLUDEFROMCAPTURE), and updated via
//! UpdateLayeredWindow(ULW_ALPHA) for zero GPU composition overhead.

#![allow(dead_code)]

use super::raster::{render_cursor_frame, SpriteCanvas, HOTSPOT_X, HOTSPOT_Y, SPRITE_HEIGHT, SPRITE_WIDTH};
use crate::ui::CursorMode;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WDA_EXCLUDEFROMCAPTURE: u32 = 0x00000011;

const WM_CURSOR_UPDATE: u32 = WM_USER + 301;
const WM_CURSOR_MOVE: u32 = WM_USER + 302;
const WM_CURSOR_HIDE: u32 = WM_USER + 303;
const WM_CURSOR_DESTROY: u32 = WM_USER + 304;

unsafe extern "system" fn cursor_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => 1,
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub struct SpriteState {
    pub x: i32,
    pub y: i32,
    pub alpha: u8,
    pub mode: CursorMode,
    pub ripple: Option<f32>,
    pub badge: Option<String>,
    pub visible: bool,
}

pub struct NativeCursorSprite {
    hwnd: Option<usize>,
    state: Arc<Mutex<SpriteState>>,
}

impl NativeCursorSprite {
    pub fn spawn() -> Self {
        let state = Arc::new(Mutex::new(SpriteState {
            x: -200,
            y: -200,
            alpha: 0,
            mode: CursorMode::Idle,
            ripple: None,
            badge: None,
            visible: false,
        }));

        let (hwnd_tx, hwnd_rx) = channel::<Option<usize>>();
        let state_clone = Arc::clone(&state);

        thread::spawn(move || {
            unsafe {
                let hinstance = GetModuleHandleW(std::ptr::null());
                let class_name: Vec<u16> = "Nexa_Virtual_Cursor\0".encode_utf16().collect();

                let mut wc: WNDCLASSEXW = std::mem::zeroed();
                wc.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
                wc.style = CS_HREDRAW | CS_VREDRAW;
                wc.lpfnWndProc = Some(cursor_wnd_proc);
                wc.hInstance = hinstance;
                wc.hCursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
                wc.lpszClassName = class_name.as_ptr();

                RegisterClassExW(&wc);

                let ex_style = WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_TOPMOST
                    | WS_EX_NOACTIVATE
                    | WS_EX_TOOLWINDOW;

                let hwnd = CreateWindowExW(
                    ex_style,
                    class_name.as_ptr(),
                    class_name.as_ptr(),
                    WS_POPUP,
                    -200,
                    -200,
                    SPRITE_WIDTH as i32,
                    SPRITE_HEIGHT as i32,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    hinstance,
                    std::ptr::null_mut(),
                );

                if hwnd.is_null() {
                    let _ = hwnd_tx.send(None);
                    return;
                }

                // Prevent cursor from being recorded by screen capture / Desktop Duplication
                SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);

                // Setup 32-bit Premultiplied ARGB DIB Section
                let screen_dc = GetDC(std::ptr::null_mut());
                let mem_dc = CreateCompatibleDC(screen_dc);

                let mut bmi: BITMAPINFO = std::mem::zeroed();
                bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
                bmi.bmiHeader.biWidth = SPRITE_WIDTH as i32;
                bmi.bmiHeader.biHeight = -(SPRITE_HEIGHT as i32); // Top-down DIB
                bmi.bmiHeader.biPlanes = 1;
                bmi.bmiHeader.biBitCount = 32;
                bmi.bmiHeader.biCompression = BI_RGB as u32;

                let mut bits_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
                let dib = CreateDIBSection(
                    mem_dc,
                    &bmi,
                    DIB_RGB_COLORS,
                    &mut bits_ptr,
                    std::ptr::null_mut(),
                    0,
                );

                let old_bitmap = SelectObject(mem_dc, dib);

                let mut canvas = SpriteCanvas::new(SPRITE_WIDTH, SPRITE_HEIGHT);

                // Initial render
                render_cursor_frame(&mut canvas, CursorMode::Idle, None, None, None);
                if !bits_ptr.is_null() {
                    std::ptr::copy_nonoverlapping(
                        canvas.pixels.as_ptr() as *const u8,
                        bits_ptr as *mut u8,
                        SPRITE_WIDTH * SPRITE_HEIGHT * 4,
                    );
                }

                let _ = hwnd_tx.send(Some(hwnd as usize));

                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                    match msg.message {
                        WM_CURSOR_UPDATE => {
                            let (x, y, alpha, mode, ripple, badge, visible) = {
                                if let Ok(st) = state_clone.lock() {
                                    (st.x, st.y, st.alpha, st.mode, st.ripple, st.badge.clone(), st.visible)
                                } else {
                                    continue;
                                }
                            };

                            if !visible || alpha == 0 {
                                ShowWindow(hwnd, SW_HIDE);
                                continue;
                            }

                            // Re-render frame
                            render_cursor_frame(&mut canvas, mode, ripple, badge.as_deref(), Some((x, y)));

                            if !bits_ptr.is_null() {
                                std::ptr::copy_nonoverlapping(
                                    canvas.pixels.as_ptr() as *const u8,
                                    bits_ptr as *mut u8,
                                    SPRITE_WIDTH * SPRITE_HEIGHT * 4,
                                );
                            }

                            let mut ppt_dst = POINT {
                                x: x - HOTSPOT_X,
                                y: y - HOTSPOT_Y,
                            };
                            let mut size = SIZE {
                                cx: SPRITE_WIDTH as i32,
                                cy: SPRITE_HEIGHT as i32,
                            };
                            let mut ppt_src = POINT { x: 0, y: 0 };
                            let mut blend = BLENDFUNCTION {
                                BlendOp: AC_SRC_OVER as u8,
                                BlendFlags: 0,
                                SourceConstantAlpha: alpha,
                                AlphaFormat: AC_SRC_ALPHA as u8,
                            };

                            UpdateLayeredWindow(
                                hwnd,
                                screen_dc,
                                &mut ppt_dst,
                                &mut size,
                                mem_dc,
                                &mut ppt_src,
                                0,
                                &mut blend,
                                ULW_ALPHA,
                            );

                            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                        }
                        WM_CURSOR_MOVE => {
                            let (x, y, alpha, visible) = {
                                if let Ok(st) = state_clone.lock() {
                                    (st.x, st.y, st.alpha, st.visible)
                                } else {
                                    continue;
                                }
                            };

                            if visible && alpha > 0 {
                                let mut ppt_dst = POINT {
                                    x: x - HOTSPOT_X,
                                    y: y - HOTSPOT_Y,
                                };
                                let mut size = SIZE {
                                    cx: SPRITE_WIDTH as i32,
                                    cy: SPRITE_HEIGHT as i32,
                                };
                                let mut ppt_src = POINT { x: 0, y: 0 };
                                let mut blend = BLENDFUNCTION {
                                    BlendOp: AC_SRC_OVER as u8,
                                    BlendFlags: 0,
                                    SourceConstantAlpha: alpha,
                                    AlphaFormat: AC_SRC_ALPHA as u8,
                                };

                                UpdateLayeredWindow(
                                    hwnd,
                                    screen_dc,
                                    &mut ppt_dst,
                                    &mut size,
                                    mem_dc,
                                    &mut ppt_src,
                                    0,
                                    &mut blend,
                                    ULW_ALPHA,
                                );
                            }
                        }
                        WM_CURSOR_HIDE => {
                            ShowWindow(hwnd, SW_HIDE);
                        }
                        WM_CURSOR_DESTROY => {
                            SelectObject(mem_dc, old_bitmap);
                            DeleteObject(dib);
                            DeleteDC(mem_dc);
                            ReleaseDC(std::ptr::null_mut(), screen_dc);
                            DestroyWindow(hwnd);
                            break;
                        }
                        _ => {
                            TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                    }
                }
            }
        });

        let hwnd = hwnd_rx
            .recv_timeout(Duration::from_millis(500))
            .unwrap_or(None);

        Self {
            hwnd,
            state,
        }
    }

    /// Redraw the frame and update position / alpha
    pub fn update(&self, x: i32, y: i32, alpha: u8, mode: CursorMode, ripple: Option<f32>, badge: Option<String>) {
        if let Ok(mut st) = self.state.lock() {
            st.x = x;
            st.y = y;
            st.alpha = alpha;
            st.mode = mode;
            st.ripple = ripple;
            st.badge = badge;
            st.visible = alpha > 0;
        }
        if let Some(h) = self.hwnd {
            unsafe {
                PostMessageW(h as HWND, WM_CURSOR_UPDATE, 0, 0);
            }
        }
    }

    /// Fast-path translation: move the sprite window on screen without re-rasterizing buffer
    pub fn move_to(&self, x: i32, y: i32, alpha: u8) {
        if let Ok(mut st) = self.state.lock() {
            st.x = x;
            st.y = y;
            st.alpha = alpha;
            st.visible = alpha > 0;
        }
        if let Some(h) = self.hwnd {
            unsafe {
                PostMessageW(h as HWND, WM_CURSOR_MOVE, 0, 0);
            }
        }
    }

    /// Hide the cursor sprite immediately
    pub fn hide(&self) {
        if let Ok(mut st) = self.state.lock() {
            st.visible = false;
            st.alpha = 0;
        }
        if let Some(h) = self.hwnd {
            unsafe {
                PostMessageW(h as HWND, WM_CURSOR_HIDE, 0, 0);
            }
        }
    }
}

impl Drop for NativeCursorSprite {
    fn drop(&mut self) {
        if let Some(h) = self.hwnd {
            unsafe {
                PostMessageW(h as HWND, WM_CURSOR_DESTROY, 0, 0);
            }
        }
    }
}
