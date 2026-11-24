#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <commctrl.h>
#include <dwmapi.h>
#include <uxtheme.h>

/* NSIS's public page plug-in messages (Examples/Plugin/nsis/api.h). */
#define NSIS_OUTER_NEXT   (WM_USER + 0x08)
#define NSIS_CUSTOM_READY (WM_USER + 0x0d)
#define MUI_BACK 3
#define MUI_BRANDING_BACKGROUND 1028
#define MUI_HEADER_BACKGROUND 1034
#define MUI_HEADER_SUBTITLE 1038
#define MUI_STANDARD_LINE 1035
#define MUI_HEADER_LINE 1036
#define MUI_FULLWINDOW_LINE 1045
#define MUI_SIDEBAR 1200

/* Color order: page, text, muted, accent, on accent, hover accent, pressed
 * accent, outline, strong outline, field/disabled fill, hover fill, pressed fill. */
typedef struct {
    COLORREF page, text, muted, accent, on_accent, accent_hot, accent_down;
    COLORREF line, edge, field, hot, down;
} Palette;

static const Palette palettes[] = {
    { RGB(255,255,255), RGB(22,28,40), RGB(70,83,105), RGB(55,88,206),
      RGB(255,255,255), RGB(43,73,191), RGB(35,59,158), RGB(211,218,230),
      RGB(115,128,150), RGB(243,246,251), RGB(240,244,253), RGB(226,233,249) },
    { RGB(22,26,34), RGB(242,244,250), RGB(186,196,213), RGB(153,174,255),
      RGB(22,26,34), RGB(178,194,255), RGB(134,157,249), RGB(53,62,77),
      RGB(110,124,149), RGB(33,40,53), RGB(39,48,63), RGB(49,60,79) }
};

typedef enum {
    ROLE_DIALOG = 1, ROLE_BUTTON, ROLE_ART, ROLE_ACCENT, ROLE_MUTED,
    ROLE_LINE, ROLE_SURFACE, ROLE_LIST, ROLE_TREE, ROLE_PROGRESS
} Role;

/* One wizard on its own UI thread. The plug-in keeps this module loaded until
 * NSIS destroys the wizard; a page or control never owns a shared GDI object. */
static struct {
    HWND wizard;
    UINT scan_message;
    BOOL scan_queued, refreshing, dark, high_contrast;
    HBRUSH page[2], field[2];
    HBITMAP art[2];
} theme;

static LRESULT CALLBACK SkinProc(HWND, UINT, WPARAM, LPARAM, UINT_PTR, DWORD_PTR);

static const Palette *palette(void)
{
    return &palettes[theme.dark];
}

static HBRUSH page_brush(void)
{
    return theme.high_contrast ? GetSysColorBrush(COLOR_BTNFACE) : theme.page[theme.dark];
}

static Role role_of(HWND hwnd)
{
    DWORD_PTR role = 0;
    GetWindowSubclass(hwnd, SkinProc, 1, &role);
    return (Role)role;
}

static BOOL skin(HWND hwnd, Role role)
{
    return SetWindowSubclass(hwnd, SkinProc, 1, (DWORD_PTR)role);
}

static BOOL class_is(HWND hwnd, LPCWSTR expected)
{
    WCHAR name[32];
    return GetClassNameW(hwnd, name, ARRAYSIZE(name)) && !lstrcmpiW(name, expected);
}

static int scale_px(HWND hwnd, int px)
{
    typedef UINT (WINAPI *DpiForWindow)(HWND);
    static DpiForWindow get_dpi;
    static BOOL resolved;
    UINT dpi = 0;
    if (!resolved) {
        get_dpi = (DpiForWindow)GetProcAddress(GetModuleHandleW(L"user32.dll"), "GetDpiForWindow");
        resolved = TRUE;
    }
    if (get_dpi) dpi = get_dpi(hwnd);
    if (!dpi) {
        HDC dc = GetDC(hwnd);
        if (dc) {
            dpi = (UINT)GetDeviceCaps(dc, LOGPIXELSY);
            ReleaseDC(hwnd, dc);
        }
    }
    return dpi ? MulDiv(px, (int)dpi, 96) : px;
}

static void fill(HDC dc, const RECT *r, COLORREF color)
{
    COLORREF old = SetDCBrushColor(dc, color);
    FillRect(dc, r, (HBRUSH)GetStockObject(DC_BRUSH));
    SetDCBrushColor(dc, old);
}

static void frame(HDC dc, const RECT *r, COLORREF color)
{
    COLORREF old = SetDCBrushColor(dc, color);
    FrameRect(dc, r, (HBRUSH)GetStockObject(DC_BRUSH));
    SetDCBrushColor(dc, old);
}

static void shape(HDC dc, const RECT *r, int radius, BOOL round,
                  COLORREF bg, COLORREF edge)
{
    HGDIOBJ brush = SelectObject(dc, GetStockObject(DC_BRUSH));
    HGDIOBJ pen = SelectObject(dc, GetStockObject(DC_PEN));
    COLORREF old_bg = SetDCBrushColor(dc, bg);
    COLORREF old_edge = SetDCPenColor(dc, edge);
    if (round) Ellipse(dc, r->left, r->top, r->right, r->bottom);
    else RoundRect(dc, r->left, r->top, r->right, r->bottom, radius, radius);
    SetDCPenColor(dc, old_edge);
    SetDCBrushColor(dc, old_bg);
    SelectObject(dc, pen);
    SelectObject(dc, brush);
}

static void paint_art(HDC dc, const RECT *r)
{
    HBITMAP bitmap = theme.high_contrast ? NULL : theme.art[theme.dark];
    BITMAP bm;
    int w = r->right - r->left, h = r->bottom - r->top;
    if (bitmap && w > 0 && h > 0 && GetObjectW(bitmap, sizeof(bm), &bm) == sizeof(bm)
        && bm.bmWidth > 0 && bm.bmHeight > 0) {
        HDC src = CreateCompatibleDC(dc);
        if (src) {
            int sw = bm.bmWidth, sh = bm.bmHeight;
            HGDIOBJ old_bitmap = SelectObject(src, bitmap);
            BOOL painted = FALSE;
            if (old_bitmap) {
                /* Cover the pane without distorting the artwork. Use a centered
                 * source crop so the destination is clipped exactly to the static. */
                int saved = SaveDC(dc);
                if ((LONGLONG)sw * h > (LONGLONG)sh * w) sw = max(1, MulDiv(sh, w, h));
                else sh = max(1, MulDiv(sw, h, w));
                if (saved) {
                    SetStretchBltMode(dc, HALFTONE);
                    SetBrushOrgEx(dc, 0, 0, NULL);
                    if (GetLayout(dc) & LAYOUT_RTL)
                        SetLayout(dc, LAYOUT_RTL | LAYOUT_BITMAPORIENTATIONPRESERVED);
                }
                painted = StretchBlt(dc, r->left, r->top, w, h, src,
                    (bm.bmWidth - sw) / 2, (bm.bmHeight - sh) / 2, sw, sh, SRCCOPY);
                if (saved) RestoreDC(dc, saved);
                SelectObject(src, old_bitmap);
            }
            DeleteDC(src);
            if (painted) return;
        }
    }
    FillRect(dc, r, page_brush());
}

static BOOL text_button(LONG_PTR style)
{
    if (style & (BS_ICON | BS_BITMAP | BS_PUSHLIKE)) return FALSE;
    switch (style & BS_TYPEMASK) {
    case BS_PUSHBUTTON: case BS_DEFPUSHBUTTON: case BS_GROUPBOX:
    case BS_CHECKBOX: case BS_AUTOCHECKBOX: case BS_3STATE: case BS_AUTO3STATE:
    case BS_RADIOBUTTON: case BS_AUTORADIOBUTTON:
        return TRUE;
    default:
        return FALSE;
    }
}

static RECT caption_rect(HDC dc, WCHAR *caption, RECT area, UINT format, LONG_PTR style)
{
    RECT text = area;
    DrawTextW(dc, caption, -1, &text, format | DT_CALCRECT);
    if ((style & BS_VCENTER) == BS_BOTTOM) text.top = area.bottom - (text.bottom - text.top);
    else if ((style & BS_VCENTER) != BS_TOP)
        text.top = area.top + max(0, (area.bottom - area.top - (text.bottom - text.top)) / 2);
    text.bottom = area.bottom;
    text.right = area.right;
    return text;
}

static void paint_button(HWND hwnd, HDC dc, RECT r)
{
    const Palette *p = palette();
    LONG_PTR style = GetWindowLongPtrW(hwnd, GWL_STYLE);
    LONG_PTR exstyle = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    LRESULT state = SendMessageW(hwnd, BM_GETSTATE, 0, 0);
    LRESULT ui = SendMessageW(hwnd, WM_QUERYUISTATE, 0, 0);
    BOOL enabled = IsWindowEnabled(hwnd), pressed = enabled && (state & BST_PUSHED);
    BOOL hot = enabled && (state & BST_HOT);
    BOOL focus = enabled && GetFocus() == hwnd && !(ui & UISF_HIDEFOCUS);
    int type = (int)(style & BS_TYPEMASK);
    UINT format = (ui & UISF_HIDEACCEL) ? DT_HIDEPREFIX : 0;
    COLORREF fg = enabled ? p->text : p->muted;
    WCHAR caption[1024] = L"";
    HFONT font = (HFONT)SendMessageW(hwnd, WM_GETFONT, 0, 0);
    int gap = scale_px(hwnd, 4);
    int saved = SaveDC(dc);
    RECT label = r, focus_rect = r;
    if (!saved) return;

    GetWindowTextW(hwnd, caption, ARRAYSIZE(caption));
    SelectObject(dc, font ? (HGDIOBJ)font : GetStockObject(DEFAULT_GUI_FONT));
    SetBkMode(dc, TRANSPARENT);
    if (exstyle & WS_EX_RTLREADING) format |= DT_RTLREADING;
    format |= (style & BS_MULTILINE) ? DT_WORDBREAK : DT_SINGLELINE;

    if (type == BS_GROUPBOX) {
        RECT bounds = r, band;
        TEXTMETRICW metrics = { 0 };
        GetTextMetricsW(dc, &metrics);
        bounds.top += metrics.tmHeight / 2;
        frame(dc, &bounds, p->line);
        label.left += 2 * gap;
        label.right -= 2 * gap;
        band = label;
        if (caption[0]) {
            DrawTextW(dc, caption, -1, &band, format | DT_CALCRECT);
            band.left -= gap;
            band.right = min(label.right, band.right + gap);
            fill(dc, &band, p->page);
            label.bottom = min(label.bottom, band.bottom);
        }
        SetTextColor(dc, fg);
        DrawTextW(dc, caption, -1, &label, format);
        RestoreDC(dc, saved);
        return;
    }

    fill(dc, &r, p->page);
    if (type == BS_PUSHBUTTON || type == BS_DEFPUSHBUTTON) {
        BOOL primary = GetParent(hwnd) == theme.wizard && GetDlgCtrlID(hwnd) == IDOK;
        COLORREF bg = enabled ? (pressed ? p->down : hot ? p->hot : p->page) : p->field;
        COLORREF border = enabled && (type == BS_DEFPUSHBUTTON || focus) ? p->accent : p->line;
        if (primary && enabled) {
            bg = pressed ? p->accent_down : hot ? p->accent_hot : p->accent;
            border = bg;
            fg = p->on_accent;
        }
        shape(dc, &r, scale_px(hwnd, 7), FALSE, bg, border);
        InflateRect(&focus_rect, -gap + 1, -gap + 1);
        label.left += gap;
        label.right -= gap;
        switch (style & BS_CENTER) {
        case BS_LEFT: format |= DT_LEFT; break;
        case BS_RIGHT: format |= DT_RIGHT; break;
        default: format |= DT_CENTER; break;
        }
        if ((style & BS_MULTILINE) == 0 && (style & BS_VCENTER) != BS_TOP &&
            (style & BS_VCENTER) != BS_BOTTOM) format |= DT_VCENTER;
        else label = caption_rect(dc, caption, label, format, style);
    } else {
        LRESULT check = SendMessageW(hwnd, BM_GETCHECK, 0, 0);
        BOOL right = (style & BS_LEFTTEXT) || (exstyle & WS_EX_RIGHT);
        BOOL radio = type == BS_RADIOBUTTON || type == BS_AUTORADIOBUTTON;
        int side = min(r.bottom - r.top, scale_px(hwnd, 13));
        RECT box = r;
        COLORREF edge = enabled ? (hot || focus ? p->accent : p->edge) : p->line;
        COLORREF bg = enabled ? (pressed ? p->down : p->field) : p->page;
        box.top += (r.bottom - r.top - side) / 2;
        box.bottom = box.top + side;
        if (right) {
            box.left = r.right - side;
            label.right = box.left - gap;
            format |= DT_RIGHT;
        } else {
            box.right = r.left + side;
            label.left = box.right + gap;
        }
        if (!radio && check != BST_UNCHECKED && enabled) bg = edge = p->accent;
        shape(dc, &box, scale_px(hwnd, 3), radio, bg, edge);
        if (check != BST_UNCHECKED && side >= 6) {
            COLORREF ink = !enabled ? p->muted : radio ? p->accent : p->on_accent;
            if (radio) {
                InflateRect(&box, -side / 4, -side / 4);
                shape(dc, &box, 0, TRUE, ink, ink);
            } else if (check == BST_INDETERMINATE) {
                RECT dash = { box.left + side / 4, box.top + side / 2 - 1,
                              box.right - side / 4, box.top + side / 2 + 1 };
                fill(dc, &dash, ink);
            } else {
                HPEN pen = CreatePen(PS_SOLID, max(1, scale_px(hwnd, 2)), ink);
                if (pen) {
                    HGDIOBJ old = SelectObject(dc, pen);
                    POINT points[] = {
                        { box.left + side / 4, box.top + side / 2 },
                        { box.left + side * 2 / 5, box.top + side * 3 / 4 },
                        { box.right - side / 5, box.top + side / 4 }
                    };
                    Polyline(dc, points, ARRAYSIZE(points));
                    SelectObject(dc, old);
                    DeleteObject(pen);
                }
            }
        }
        label = caption_rect(dc, caption, label, format, style);
        focus_rect = label;
        DrawTextW(dc, caption, -1, &focus_rect, format | DT_CALCRECT);
        if (right) {
            focus_rect.left += label.right - focus_rect.right;
            focus_rect.right = label.right;
        }
        InflateRect(&focus_rect, 1, 1);
        IntersectRect(&focus_rect, &focus_rect, &r);
    }
    SetTextColor(dc, fg);
    DrawTextW(dc, caption, -1, &label, format);
    if (focus) {
        SetBkColor(dc, p->page);
        DrawFocusRect(dc, &focus_rect);
    }
    RestoreDC(dc, saved);
}

static void paint_control(HWND hwnd, HDC dc, Role role)
{
    RECT r;
    GetClientRect(hwnd, &r);
    switch (role) {
    case ROLE_BUTTON: paint_button(hwnd, dc, r); break;
    case ROLE_ART: paint_art(dc, &r); break;
    case ROLE_ACCENT: fill(dc, &r, theme.high_contrast ? GetSysColor(COLOR_HIGHLIGHT) : palette()->accent); break;
    case ROLE_LINE:
        FillRect(dc, &r, page_brush());
        r.top += (r.bottom - r.top) / 2;
        r.bottom = r.top + 1;
        fill(dc, &r, theme.high_contrast ? GetSysColor(COLOR_BTNTEXT) : palette()->line);
        break;
    case ROLE_SURFACE: FillRect(dc, &r, page_brush()); break;
    default: break;
    }
}

static LRESULT control_colors(UINT msg, HDC dc, HWND control)
{
    BOOL edit = class_is(control, L"Edit"); /* WM_CTLCOLORSTATIC also covers read-only edits. */
    BOOL field = msg == WM_CTLCOLOREDIT || msg == WM_CTLCOLORLISTBOX || edit;
    BOOL muted = role_of(control) == ROLE_MUTED ||
        (GetParent(control) == theme.wizard && GetDlgCtrlID(control) == MUI_HEADER_SUBTITLE);
    COLORREF bg, fg;
    if (theme.high_contrast) {
        bg = GetSysColor(field ? COLOR_WINDOW : COLOR_BTNFACE);
        fg = GetSysColor(!IsWindowEnabled(control) ? COLOR_GRAYTEXT : field ? COLOR_WINDOWTEXT : COLOR_BTNTEXT);
    } else {
        bg = field ? palette()->field : palette()->page;
        fg = muted || !IsWindowEnabled(control) ? palette()->muted : palette()->text;
    }
    SetTextColor(dc, fg);
    SetBkColor(dc, bg);
    SetBkMode(dc, field ? OPAQUE : TRANSPARENT);
    if (!field) return (LRESULT)page_brush();
    return (LRESULT)(theme.high_contrast ? GetSysColorBrush(COLOR_WINDOW) : theme.field[theme.dark]);
}

static void color_common_control(HWND hwnd, Role role)
{
    const Palette *p = palette();
    COLORREF bg = theme.high_contrast ? GetSysColor(COLOR_WINDOW) : p->field;
    COLORREF fg = theme.high_contrast ? GetSysColor(COLOR_WINDOWTEXT) : p->text;
    switch (role) {
    case ROLE_LIST:
        SendMessageW(hwnd, LVM_SETBKCOLOR, 0, bg);
        SendMessageW(hwnd, LVM_SETTEXTBKCOLOR, 0, bg);
        SendMessageW(hwnd, LVM_SETTEXTCOLOR, 0, fg);
        break;
    case ROLE_TREE:
        SendMessageW(hwnd, TVM_SETBKCOLOR, 0, bg);
        SendMessageW(hwnd, TVM_SETTEXTCOLOR, 0, fg);
        break;
    case ROLE_PROGRESS:
        /* The themed progress bar ignores the color messages. In high contrast
         * restore its system rendering; never change range, state or position. */
        SetWindowTheme(hwnd, theme.high_contrast ? NULL : L"", theme.high_contrast ? NULL : L"");
        SendMessageW(hwnd, PBM_SETBKCOLOR, 0, theme.high_contrast ? CLR_DEFAULT : bg);
        SendMessageW(hwnd, PBM_SETBARCOLOR, 0, theme.high_contrast ? CLR_DEFAULT : p->accent);
        break;
    default: break;
    }
}

static BOOL is_sidebar(HWND hwnd, LONG_PTR style)
{
    RECT image, page;
    HWND parent = GetParent(hwnd);
    if (GetDlgCtrlID(hwnd) != MUI_SIDEBAR || (style & SS_TYPEMASK) != SS_BITMAP ||
        GetParent(parent) != theme.wizard || !class_is(parent, L"#32770")) return FALSE;
    if (!GetClientRect(hwnd, &image) || !GetClientRect(parent, &page)) return FALSE;
    /* The stock MUI welcome/finish sidebar takes up a tall, narrow portion of
     * the inner dialog. ID 1200 alone is also used by other nsDialogs pages. */
    return image.right > 0 && image.right * 5 >= page.right && image.right * 2 <= page.right &&
        image.bottom * 4 >= page.bottom * 3 && image.bottom > image.right;
}

static BOOL CALLBACK skin_child(HWND hwnd, LPARAM recolor)
{
    Role role = role_of(hwnd);
    LONG_PTR style;
    if (role) {
        if (recolor) color_common_control(hwnd, role);
        return TRUE;
    }
    style = GetWindowLongPtrW(hwnd, GWL_STYLE);
    if (class_is(hwnd, L"#32770")) role = ROLE_DIALOG;
    else if (class_is(hwnd, L"Button") && text_button(style)) role = ROLE_BUTTON;
    else if (class_is(hwnd, WC_LISTVIEWW)) role = ROLE_LIST;
    else if (class_is(hwnd, WC_TREEVIEWW)) role = ROLE_TREE;
    else if (class_is(hwnd, PROGRESS_CLASSW)) role = ROLE_PROGRESS;
    else if (class_is(hwnd, L"Static")) {
        if (is_sidebar(hwnd, style)) role = ROLE_ART;
        else if (GetParent(hwnd) == theme.wizard) {
            int id = GetDlgCtrlID(hwnd);
            if (id == MUI_STANDARD_LINE || id == MUI_FULLWINDOW_LINE || id == MUI_HEADER_LINE)
                role = ROLE_LINE;
            else if (id == MUI_HEADER_BACKGROUND || id == MUI_BRANDING_BACKGROUND) role = ROLE_SURFACE; /* MUI header/branding fills. */
        }
    }
    if (role && skin(hwnd, role)) {
        if (role == ROLE_LINE) {
            LONG_PTR exstyle = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            const LONG_PTR edges = WS_EX_STATICEDGE | WS_EX_CLIENTEDGE | WS_EX_WINDOWEDGE;
            /* User32 implements SS_ETCHEDHORZ with a native non-client edge.
             * Reclaim the whole separator so the system edge cannot stay white. */
            if ((exstyle & edges) || (style & WS_BORDER)) {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, exstyle & ~edges);
                if (style & WS_BORDER) SetWindowLongPtrW(hwnd, GWL_STYLE, style & ~(LONG_PTR)WS_BORDER);
                SetWindowPos(hwnd, NULL, 0, 0, 0, 0,
                             SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
            }
        }
        color_common_control(hwnd, role);
    }
    return TRUE;
}

static void layout_footer(HWND wizard)
{
    static const int ids[] = { IDOK, IDCANCEL, MUI_BACK };
    static const int lines[] = { MUI_FULLWINDOW_LINE, MUI_STANDARD_LINE };
    RECT client;
    int target = scale_px(wizard, 28);
    UINT i, j;
    if (target <= 0 || !GetClientRect(wizard, &client)) return;
    for (i = 0; i < ARRAYSIZE(ids); ++i) {
        HWND button = GetDlgItem(wizard, ids[i]);
        RECT r;
        int y;
        if (role_of(button) != ROLE_BUTTON || !GetWindowRect(button, &r)) continue;
        MapWindowPoints(HWND_DESKTOP, wizard, (POINT *)&r, 2);
        y = (r.top + r.bottom - target) / 2;
        for (j = 0; j < ARRAYSIZE(lines); ++j) {
            HWND line = GetDlgItem(wizard, lines[j]);
            RECT separator;
            int band;
            if (!(GetWindowLongPtrW(line, GWL_STYLE) & WS_VISIBLE) || !GetWindowRect(line, &separator)) continue;
            MapWindowPoints(HWND_DESKTOP, wizard, (POINT *)&separator, 2);
            band = client.bottom - separator.bottom;
            if (separator.bottom <= r.top && band >= target + scale_px(wizard, 4) &&
                band <= target + scale_px(wizard, 34)) {
                y = separator.bottom + (band - target) / 2;
                break;
            }
        }
        if (target >= r.bottom - r.top && y >= client.top && y + target <= client.bottom &&
            (y != r.top || target != r.bottom - r.top)) {
            SetWindowPos(button, NULL, r.left, y, r.right - r.left, target,
                         SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
        }
    }
}

static void scan_wizard(HWND wizard, BOOL recolor)
{
    EnumChildWindows(wizard, skin_child, recolor);
    layout_footer(wizard);
}

static void queue_scan(HWND hwnd)
{
    if (theme.scan_message && !theme.scan_queued)
        theme.scan_queued = PostMessageW(hwnd, theme.scan_message, 0, 0);
}

static void refresh_theme(HWND hwnd)
{
    HIGHCONTRASTW hc = { sizeof(hc), 0, NULL };
    DWORD light = 1, size = sizeof(light);
    BOOL dark_title;
    if (theme.refreshing) return;
    theme.refreshing = TRUE;
    theme.high_contrast = SystemParametersInfoW(SPI_GETHIGHCONTRAST, sizeof(hc), &hc, 0) &&
        (hc.dwFlags & HCF_HIGHCONTRASTON);
    if (RegGetValueW(HKEY_CURRENT_USER,
            L"Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",
            L"AppsUseLightTheme", RRF_RT_REG_DWORD, NULL, &light, &size) != ERROR_SUCCESS) light = 1;
    theme.dark = !light && !theme.high_contrast;
    dark_title = theme.dark;
    /* DWMWA_USE_IMMERSIVE_DARK_MODE. Older Windows can reject the hint. The
     * non-client frame remains owned by Windows, including in high contrast. */
    DwmSetWindowAttribute(hwnd, 20, &dark_title, sizeof(dark_title));
    scan_wizard(hwnd, TRUE);
    RedrawWindow(hwnd, NULL, NULL, RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_FRAME);
    theme.refreshing = FALSE;
}

static void release_resources(void)
{
    int i;
    for (i = 0; i < 2; ++i) {
        if (theme.page[i]) DeleteObject(theme.page[i]);
        if (theme.field[i]) DeleteObject(theme.field[i]);
        if (theme.art[i]) DeleteObject(theme.art[i]);
        theme.page[i] = theme.field[i] = NULL;
        theme.art[i] = NULL;
    }
}

static LRESULT CALLBACK SkinProc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp,
                                 UINT_PTR id, DWORD_PTR data)
{
    Role role = (Role)data;
    BOOL wizard = hwnd == theme.wizard;
    if (msg == WM_NCDESTROY) {
        LRESULT result;
        RemoveWindowSubclass(hwnd, SkinProc, id);
        result = DefSubclassProc(hwnd, msg, wp, lp);
        if (wizard) {
            theme.wizard = NULL;
            theme.scan_queued = FALSE;
            release_resources();
        }
        return result;
    }
    if (role == ROLE_DIALOG) {
        switch (msg) {
        case WM_CTLCOLORDLG: return (LRESULT)page_brush();
        case WM_CTLCOLORSTATIC: case WM_CTLCOLOREDIT: case WM_CTLCOLORLISTBOX: case WM_CTLCOLORBTN:
            return control_colors(msg, (HDC)wp, (HWND)lp);
        case WM_ERASEBKGND: {
            RECT r;
            GetClientRect(hwnd, &r);
            FillRect((HDC)wp, &r, page_brush());
            return 1;
        }
        case WM_NOTIFY:
            /* A v6 Button can also paint during a native state change, not just
             * WM_PAINT. Keep that rendering consistent with the subclass. */
            if (!theme.high_contrast && lp) {
                const NMHDR *hdr = (const NMHDR *)lp;
                if (hdr->code == NM_CUSTOMDRAW && role_of(hdr->hwndFrom) == ROLE_BUTTON &&
                    text_button(GetWindowLongPtrW(hdr->hwndFrom, GWL_STYLE))) {
                    const NMCUSTOMDRAW *draw = (const NMCUSTOMDRAW *)lp;
                    if (draw->dwDrawStage == CDDS_PREPAINT) {
                        paint_control(hdr->hwndFrom, draw->hdc, ROLE_BUTTON);
                        return CDRF_SKIPDEFAULT;
                    }
                }
            }
            break;
        default: break;
        }
    }
    if (wizard) {
        if (msg == theme.scan_message && theme.scan_message) {
            theme.scan_queued = FALSE;
            scan_wizard(hwnd, FALSE);
            return 0;
        }
        switch (msg) {
        case WM_SETTINGCHANGE: case WM_THEMECHANGED: case WM_SYSCOLORCHANGE:
            refresh_theme(hwnd);
            break;
        case WM_WINDOWPOSCHANGING:
            if (lp && (((const WINDOWPOS *)lp)->flags & SWP_SHOWWINDOW)) scan_wizard(hwnd, FALSE);
            break;
        case WM_DPICHANGED: case WM_SIZE:
            queue_scan(hwnd);
            break;
        case NSIS_CUSTOM_READY:
            /* nsDialogs has already constructed the page; subclass before
             * NSIS shows it so the stock bitmap is never painted on entry. */
            scan_wizard(hwnd, FALSE);
            break;
        case NSIS_OUTER_NEXT: {
            LRESULT result;
            /* Custom pages can run their own message loop within this call. */
            queue_scan(hwnd);
            result = DefSubclassProc(hwnd, msg, wp, lp);
            if (theme.wizard == hwnd) scan_wizard(hwnd, FALSE);
            return result;
        }
        default: break;
        }
    }
    if (role == ROLE_ART || role == ROLE_ACCENT || role == ROLE_LINE || role == ROLE_SURFACE ||
        (role == ROLE_BUTTON && !theme.high_contrast && text_button(GetWindowLongPtrW(hwnd, GWL_STYLE)))) {
        switch (msg) {
        case WM_ERASEBKGND: return 1;
        case WM_PAINT: {
            PAINTSTRUCT ps;
            HDC dc = BeginPaint(hwnd, &ps);
            if (dc) paint_control(hwnd, dc, role);
            EndPaint(hwnd, &ps);
            return 0;
        }
        case WM_PRINTCLIENT:
            if (wp) paint_control(hwnd, (HDC)wp, role);
            return 0;
        default: break;
        }
        if (role == ROLE_BUTTON) {
            switch (msg) {
            case BM_SETSTATE: case BM_SETCHECK: case BM_SETSTYLE: case BM_CLICK:
            case WM_LBUTTONDOWN: case WM_LBUTTONUP: case WM_KEYDOWN: case WM_KEYUP:
            case WM_CAPTURECHANGED: case WM_CANCELMODE: case WM_SETTEXT:
            case WM_SETFONT: case WM_SETFOCUS: case WM_KILLFOCUS: case WM_ENABLE:
            case WM_UPDATEUISTATE: case WM_MOUSELEAVE: case WM_MOUSEMOVE: {
                LRESULT before = msg == WM_MOUSEMOVE ? SendMessageW(hwnd, BM_GETSTATE, 0, 0) : 0;
                LRESULT result = DefSubclassProc(hwnd, msg, wp, lp);
                if (msg != WM_MOUSEMOVE || before != SendMessageW(hwnd, BM_GETSTATE, 0, 0))
                    InvalidateRect(hwnd, NULL, FALSE);
                return result;
            }
            default: break;
            }
        }
    }
    return DefSubclassProc(hwnd, msg, wp, lp);
}

#ifdef __cplusplus
extern "C" {
#endif

void WINAPI ApplyInstallerTheme(HWND wizard)
{
    HMODULE module = NULL;
    int i;
    if (theme.wizard || !wizard || GetWindowThreadProcessId(wizard, NULL) != GetCurrentThreadId()) return;
    GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                       (LPCWSTR)&theme, &module);
    for (i = 0; i < 2; ++i) {
        theme.page[i] = CreateSolidBrush(palettes[i].page);
        theme.field[i] = CreateSolidBrush(palettes[i].field);
        if (module) theme.art[i] = (HBITMAP)LoadImageW(module, MAKEINTRESOURCEW(101 + i),
                    IMAGE_BITMAP, 0, 0, LR_CREATEDIBSECTION);
        if (!theme.page[i] || !theme.field[i]) {
            release_resources();
            return;
        }
    }
    if (!skin(wizard, ROLE_DIALOG)) {
        release_resources();
        return;
    }
    theme.wizard = wizard;
    theme.scan_message = RegisterWindowMessageW(L"GlossMod.AgentModManager.InstallerTheme.Scan");
    refresh_theme(wizard);
    queue_scan(wizard);
}

void WINAPI SetWelcomeControls(HWND art, HWND heading, HWND body, HWND kicker, HWND version, HWND accent)
{
    HWND controls[] = { art, heading, body, kicker, version, accent };
    const Role roles[] = { ROLE_ART, 0, ROLE_MUTED, ROLE_MUTED, ROLE_MUTED, ROLE_ACCENT };
    UINT i;
    if (!theme.wizard || GetWindowThreadProcessId(theme.wizard, NULL) != GetCurrentThreadId()) return;
    scan_wizard(theme.wizard, FALSE);
    for (i = 0; i < ARRAYSIZE(controls); ++i) {
        HWND control = controls[i];
        if (control && IsChild(theme.wizard, control) && class_is(control, L"Static")) {
            if (roles[i]) skin(control, roles[i]);
            else if (role_of(control) == ROLE_MUTED) RemoveWindowSubclass(control, SkinProc, 1);
            InvalidateRect(control, NULL, TRUE);
        }
    }
}

#ifdef __cplusplus
}
#endif
