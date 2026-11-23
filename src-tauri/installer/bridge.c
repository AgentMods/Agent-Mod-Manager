#include <windows.h>
#include <wchar.h>

void WINAPI ApplyInstallerTheme(HWND wizard);
void WINAPI SetWelcomeControls(HWND art, HWND heading, HWND body, HWND kicker,
                              HWND version, HWND accent);

/* NSIS Unicode plug-in ABI. Arguments are pushed as strings, first arg on top. */
typedef struct Argument {
    struct Argument *next;
    WCHAR text[1];
} Argument;

static HWND pop_window(Argument **stack) {
    Argument *arg = *stack;
    HWND handle;
    if (!arg) return NULL;
    *stack = arg->next;
    handle = (HWND)(UINT_PTR)wcstoul(arg->text, NULL, 0);
    GlobalFree(arg);
    return handle;
}

__declspec(dllexport) void __cdecl Initialize(HWND parent, int size, WCHAR *variables,
                                              Argument **stack, void *extra) {
    (void)size; (void)variables; (void)stack; (void)extra;
    ApplyInstallerTheme(parent);
}

__declspec(dllexport) void __cdecl Welcome(HWND parent, int size, WCHAR *variables,
                                           Argument **stack, void *extra) {
    HWND art = pop_window(stack);
    HWND heading = pop_window(stack);
    HWND body = pop_window(stack);
    HWND kicker = pop_window(stack);
    HWND version = pop_window(stack);
    HWND accent = pop_window(stack);
    (void)parent; (void)size; (void)variables; (void)extra;
    SetWelcomeControls(art, heading, body, kicker, version, accent);
}
