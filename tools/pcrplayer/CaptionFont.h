#pragma once
#include <dwmapi.h>
#include <uxtheme.h>
#include <commctrl.h>
#include <windowsx.h>
#include <string>
#include <algorithm>
#pragma comment(lib, "dwmapi.lib")
#pragma comment(lib, "uxtheme.lib")
#pragma comment(lib, "comctl32.lib")

// The native caption buttons/styles remain intact. The caption's left side is
// a client child, so its font does not depend on Windows' system caption font.
namespace player_caption {
const UINT_PTR SubclassId = 0x50435246;
struct State { HWND child = nullptr; HFONT font = nullptr; int height = 0; bool styled = false; bool enabled = true; };
inline LRESULT CALLBACK Procedure(HWND, UINT, WPARAM, LPARAM, UINT_PTR, DWORD_PTR);
inline bool HasCaption(HWND window) {
    DWORD_PTR reference = 0;
    if (GetWindowSubclass(window, Procedure, SubclassId, &reference)
        && !reinterpret_cast<State*>(reference)->enabled) return false;
    return (GetWindowLongPtrW(window, GWL_STYLE) & WS_CAPTION) == WS_CAPTION;
}
inline int Height(HWND window) {
    DWORD_PTR reference = 0;
    return HasCaption(window) && GetWindowSubclass(window, Procedure, SubclassId, &reference)
        ? GetSystemMetrics(SM_CYCAPTION) + GetSystemMetrics(SM_CYFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER) : 0;
}
inline LRESULT CALLBACK CaptionProcedure(HWND window, UINT message, WPARAM wp, LPARAM lp) {
    if (message == WM_NCCREATE) {
        SetWindowLongPtrW(window, GWLP_USERDATA,
            reinterpret_cast<LONG_PTR>(reinterpret_cast<CREATESTRUCTW*>(lp)->lpCreateParams));
    }
    State* state = reinterpret_cast<State*>(GetWindowLongPtrW(window, GWLP_USERDATA));
    if (message == WM_NCHITTEST) {
        RECT bounds = {}; GetWindowRect(window, &bounds);
        const int grip = GetSystemMetrics(SM_CYFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER);
        return GET_Y_LPARAM(lp) - bounds.top < grip ? HTTRANSPARENT : HTCLIENT;
    }
    if (message == WM_LBUTTONDOWN || message == WM_LBUTTONDBLCLK || message == WM_RBUTTONUP) {
        POINT point = { GET_X_LPARAM(lp), GET_Y_LPARAM(lp) }; ClientToScreen(window, &point);
        ReleaseCapture();
        const UINT forwarded = message == WM_LBUTTONDOWN ? WM_NCLBUTTONDOWN
            : message == WM_LBUTTONDBLCLK ? WM_NCLBUTTONDBLCLK : WM_NCRBUTTONUP;
        SendMessageW(GetParent(window), forwarded, HTCAPTION, MAKELPARAM(point.x, point.y));
        return 0;
    }
    if (message == WM_ERASEBKGND) return 1;
    if (message == WM_PAINT && state) {
        PAINTSTRUCT paint = {};
        HDC dc = BeginPaint(window, &paint);
        RECT rc = {}; GetClientRect(window, &rc);
        HDC memory = CreateCompatibleDC(dc);
        BITMAPINFO bitmapInfo = {};
        bitmapInfo.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
        bitmapInfo.bmiHeader.biWidth = rc.right;
        bitmapInfo.bmiHeader.biHeight = -rc.bottom;
        bitmapInfo.bmiHeader.biPlanes = 1; bitmapInfo.bmiHeader.biBitCount = 32;
        void* pixels = nullptr;
        HBITMAP bitmap = CreateDIBSection(dc, &bitmapInfo, DIB_RGB_COLORS, &pixels, nullptr, 0);
        if (!memory || !bitmap) {
            if (bitmap) DeleteObject(bitmap); if (memory) DeleteDC(memory);
            EndPaint(window, &paint); return 0;
        }
        HGDIOBJ previousBitmap = SelectObject(memory, bitmap);
        const int saved = SaveDC(memory);
        HBRUSH brush = CreateSolidBrush(RGB(245, 243, 250));
        FillRect(memory, &rc, brush); DeleteObject(brush);
        SelectObject(memory, state->font);
        SetBkMode(memory, TRANSPARENT); SetTextColor(memory, RGB(36, 29, 55));
        const int iconSize = GetSystemMetrics(SM_CXSMICON);
        const int inset = (std::max)(4, state->height / 7);
        HICON icon = reinterpret_cast<HICON>(SendMessageW(GetParent(window), WM_GETICON, ICON_SMALL, 0));
        if (icon) DrawIconEx(memory, inset, (state->height - iconSize) / 2, icon,
            iconSize, iconSize, 0, nullptr, DI_NORMAL);
        RECT text = rc; text.left = inset * 2 + iconSize; text.right -= inset;
        const int length = (std::min)(GetWindowTextLengthW(GetParent(window)) + 1, 4096);
        std::wstring title(length, L'\0');
        const int count = GetWindowTextW(GetParent(window), &title[0], length);
        DrawTextW(memory, title.c_str(), count, &text, DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX);
        // GDI text writes zero alpha. Make the composed client caption opaque.
        GdiFlush();
        DWORD* rgba = static_cast<DWORD*>(pixels);
        for (int pixel = 0; pixel < rc.right * rc.bottom; ++pixel) rgba[pixel] |= 0xff000000;
        BitBlt(dc, 0, 0, rc.right, rc.bottom, memory, 0, 0, SRCCOPY);
        RestoreDC(memory, saved); SelectObject(memory, previousBitmap);
        DeleteObject(bitmap); DeleteDC(memory);
        EndPaint(window, &paint); return 0;
    }
    return DefWindowProcW(window, message, wp, lp);
}
inline void ApplyFrame(HWND window, State* state) {
    // Fullscreen toggles recreate the native frame. Reapply the DWM extension
    // when returning to a captioned window, without changing system settings.
    state->styled = true;
    const DWMNCRENDERINGPOLICY policy = DWMNCRP_ENABLED;
    DwmSetWindowAttribute(window, DWMWA_NCRENDERING_POLICY, &policy, sizeof(policy));
    const DWORD captionColor = RGB(245, 243, 250);
    DwmSetWindowAttribute(window, static_cast<DWMWINDOWATTRIBUTE>(35), &captionColor, sizeof(captionColor));
    WTA_OPTIONS options = { WTNCA_NODRAWCAPTION | WTNCA_NODRAWICON,
        WTNCA_NODRAWCAPTION | WTNCA_NODRAWICON };
    SetWindowThemeAttribute(window, WTA_NONCLIENT, &options, sizeof(options));
    MARGINS margins = { 0, 0, Height(window), 0 };
    DwmExtendFrameIntoClientArea(window, &margins);
}
inline void Layout(HWND window, State* state) {
    if (!state->child) return;
    state->height = Height(window);
    if (!state->height || IsIconic(window)) { state->styled = false; ShowWindow(state->child, SW_HIDE); return; }
    if (!state->styled) ApplyFrame(window, state);
    RECT rc = {}; GetClientRect(window, &rc);
    POINT origin = {}; ClientToScreen(window, &origin);
    RECT outer = {}; GetWindowRect(window, &outer);
    RECT buttons = {};
    int width = (std::max)(0L, rc.right - 3 * GetSystemMetrics(SM_CXSIZE));
    if (SUCCEEDED(DwmGetWindowAttribute(window, DWMWA_CAPTION_BUTTON_BOUNDS, &buttons, sizeof(buttons)))
        && buttons.left > origin.x - outer.left) width = buttons.left - (origin.x - outer.left);
    const int border = GetSystemMetrics(SM_CXFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER);
    SetWindowPos(state->child, HWND_TOP, border, 0, (std::max)(0, width - border), state->height, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    InvalidateRect(state->child, nullptr, FALSE);
}
inline LRESULT CALLBACK Procedure(HWND window, UINT message, WPARAM wp, LPARAM lp,
    UINT_PTR id, DWORD_PTR reference) {
    State* state = reinterpret_cast<State*>(reference);
    if (message == WM_NCDESTROY) {
        RemoveWindowSubclass(window, Procedure, id);
        DeleteObject(state->font); delete state;
        return DefSubclassProc(window, message, wp, lp);
    }
    if (message == WM_NCCALCSIZE && HasCaption(window)) {
        // Returning the unmodified window rect makes the whole frame client,
        // as in Microsoft's DWM custom-frame example.
        return 0;
    }
    if (message == WM_NCHITTEST && HasCaption(window)) {
        LRESULT hit = 0;
        if (DwmDefWindowProc(window, message, wp, lp, &hit) && hit != HTCLIENT && hit != HTNOWHERE) return hit;
        RECT bounds = {}; GetWindowRect(window, &bounds);
        const int x = GET_X_LPARAM(lp) - bounds.left, y = GET_Y_LPARAM(lp) - bounds.top;
        const int gripX = GetSystemMetrics(SM_CXFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER);
        const int gripY = GetSystemMetrics(SM_CYFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER);
        const bool left = x < gripX, right = x >= bounds.right - bounds.left - gripX;
        const bool top = y < gripY, bottom = y >= bounds.bottom - bounds.top - gripY;
        if (GetWindowLongPtrW(window, GWL_STYLE) & WS_THICKFRAME) {
            if (top && left) return HTTOPLEFT; if (top && right) return HTTOPRIGHT;
            if (bottom && left) return HTBOTTOMLEFT; if (bottom && right) return HTBOTTOMRIGHT;
            if (top) return HTTOP; if (bottom) return HTBOTTOM;
            if (left) return HTLEFT; if (right) return HTRIGHT;
        }
        hit = DefSubclassProc(window, message, wp, lp);
        if (hit == HTCLIENT) {
            POINT point = { GET_X_LPARAM(lp), GET_Y_LPARAM(lp) }; ScreenToClient(window, &point);
            if (point.y >= 0 && point.y < Height(window))
                return point.x < GetSystemMetrics(SM_CXSMICON) + 12 ? HTSYSMENU : HTCAPTION;
        }
        return hit;
    }
    const LRESULT result = DefSubclassProc(window, message, wp, lp);
    switch (message) {
    case WM_SETTEXT: case WM_SIZE: case WM_WINDOWPOSCHANGED: case WM_NCACTIVATE:
        Layout(window, state); break;
    }
    return result;
}
inline void SetFrameVisible(HWND window, bool enabled) {
    DWORD_PTR reference = 0;
    if (!GetWindowSubclass(window, Procedure, SubclassId, &reference)) return;
    State* state = reinterpret_cast<State*>(reference);
    if (state->enabled == enabled) return;
    state->enabled = enabled; state->styled = false;
    if (!enabled) { MARGINS empty = {}; DwmExtendFrameIntoClientArea(window, &empty); }
    SetWindowPos(window, nullptr, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
    Layout(window, state);
}
inline void AdjustContent(HWND window, RECT& rc) {
    const int height = Height(window);
    if (!height) return;
    const int borderX = GetSystemMetrics(SM_CXFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER);
    const int borderY = GetSystemMetrics(SM_CYFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER);
    rc.left += borderX; rc.right = (std::max)(rc.left, rc.right - borderX);
    rc.top += height; rc.bottom = (std::max)(rc.top, rc.bottom - borderY);
}
inline bool Install(HWND window, bool frameVisible) {
    State* state = new State; state->enabled = frameVisible;
    HDC dc = GetDC(window);
    const int dpi = dc ? GetDeviceCaps(dc, LOGPIXELSY) : 96;
    if (dc) ReleaseDC(window, dc);
    state->font = CreateFontW(-MulDiv(11, dpi, 72), 0, 0, 0, FW_SEMIBOLD,
        FALSE, FALSE, FALSE, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
        CLEARTYPE_QUALITY, DEFAULT_PITCH, L"Yu Gothic UI");
    WNDCLASSW cls = {}; cls.style = CS_DBLCLKS; cls.lpfnWndProc = CaptionProcedure;
    cls.hInstance = GetModuleHandleW(nullptr); cls.lpszClassName = L"PCRPlayerReadableCaption";
    cls.hCursor = LoadCursor(nullptr, IDC_ARROW);
    if (!state->font || (!RegisterClassW(&cls) && GetLastError() != ERROR_CLASS_ALREADY_EXISTS)
        || !SetWindowSubclass(window, Procedure, SubclassId, reinterpret_cast<DWORD_PTR>(state))) {
        if (state->font) DeleteObject(state->font); delete state; return false;
    }
    state->child = CreateWindowExW(0, cls.lpszClassName, L"", WS_CHILD | WS_CLIPSIBLINGS,
        0, 0, 0, 0, window, nullptr, cls.hInstance, state);
    if (!state->child) {
        RemoveWindowSubclass(window, Procedure, SubclassId);
        DeleteObject(state->font); delete state; return false;
    }
    ApplyFrame(window, state);
    SetWindowPos(window, nullptr, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
    Layout(window, state);
    return true;
}
}
