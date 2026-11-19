#Requires -Version 7.0

[CmdletBinding()]
param(
    [Parameter(Mandatory)] [ValidateRange(1, 2147483647)] [int] $AppPid,
    [Parameter(Mandatory)] [string] $ArchivePath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) {
    throw 'The native archive chooser adapter requires Windows.'
}
if (-not [IO.Path]::IsPathFullyQualified($ArchivePath)) {
    throw "ArchivePath must be an absolute filesystem path: '$ArchivePath'."
}
$ArchivePath = [IO.Path]::GetFullPath($ArchivePath)
if (-not [IO.File]::Exists($ArchivePath)) {
    throw "ArchivePath must name an existing file: '$ArchivePath'."
}
$app = Get-Process -Id $AppPid -ErrorAction SilentlyContinue
if ($null -eq $app -or $app.HasExited -or $AppPid -eq $PID) {
    throw "AppPid must identify the running smoke app (got $AppPid)."
}

# Target the native controls directly so messages have bounded waits and need no focus change.
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;

namespace AmmSmoke {
    public static class ArchiveChooser {
        private const int IdOk = 1;
        private const int FileNameCombo = 1148; // cmb13
        private const int FileNameEdit = 1152; // edt1
        private const int ComboEdit = 1001;
        private const uint WmSetText = 0x000C;
        private const uint WmGetText = 0x000D;
        private const uint WmCommand = 0x0111;
        private const uint AbortIfHung = 0x0002;
        private const uint MessageTimeoutMs = 1000;

        private delegate bool WindowCallback(IntPtr window, IntPtr data);

        [DllImport("user32.dll")]
        private static extern bool EnumWindows(WindowCallback callback, IntPtr data);
        [DllImport("user32.dll")]
        private static extern bool EnumChildWindows(IntPtr parent, WindowCallback callback, IntPtr data);
        [DllImport("user32.dll")]
        private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
        [DllImport("user32.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
        private static extern int GetWindowTextW(IntPtr window, StringBuilder text, int length);
        [DllImport("user32.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
        private static extern int GetClassNameW(IntPtr window, StringBuilder text, int length);
        [DllImport("user32.dll")]
        private static extern bool IsWindowVisible(IntPtr window);
        [DllImport("user32.dll")]
        private static extern bool IsWindowEnabled(IntPtr window);
        [DllImport("user32.dll")]
        private static extern bool IsChild(IntPtr parent, IntPtr window);
        [DllImport("user32.dll")]
        private static extern IntPtr GetParent(IntPtr window);
        [DllImport("user32.dll")]
        private static extern IntPtr GetDlgItem(IntPtr dialog, int id);
        [DllImport("user32.dll")]
        private static extern int GetDlgCtrlID(IntPtr window);
        [DllImport("user32.dll", CharSet = CharSet.Unicode, EntryPoint = "SendMessageTimeoutW", ExactSpelling = true, SetLastError = true)]
        private static extern IntPtr SetTextMessage(IntPtr window, uint message, UIntPtr wParam,
            string text, uint flags, uint timeout, out UIntPtr result);
        [DllImport("user32.dll", CharSet = CharSet.Unicode, EntryPoint = "SendMessageTimeoutW", ExactSpelling = true, SetLastError = true)]
        private static extern IntPtr GetTextMessage(IntPtr window, uint message, UIntPtr wParam,
            StringBuilder text, uint flags, uint timeout, out UIntPtr result);
        [DllImport("user32.dll", ExactSpelling = true, SetLastError = true)]
        private static extern bool PostMessageW(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);

        public static bool BelongsToProcess(IntPtr window, uint pid) {
            return GetWindowThreadProcessId(window, out uint actualPid) != 0 && actualPid == pid;
        }

        private static bool HasClass(IntPtr window, string expected) {
            var name = new StringBuilder(expected.Length + 2);
            GetClassNameW(window, name, name.Capacity);
            return string.Equals(name.ToString(), expected, StringComparison.OrdinalIgnoreCase);
        }

        private static bool MatchesDialog(IntPtr window, uint pid, string title) {
            if (!BelongsToProcess(window, pid) || !IsWindowVisible(window) || !HasClass(window, "#32770"))
                return false;
            var caption = new StringBuilder(title.Length + 2);
            GetWindowTextW(window, caption, caption.Capacity);
            return string.Equals(caption.ToString(), title, StringComparison.Ordinal);
        }

        public static IntPtr FindDialog(uint pid, string title) {
            IntPtr dialog = IntPtr.Zero;
            bool ambiguous = false;
            EnumWindows((window, data) => {
                if (!MatchesDialog(window, pid, title)) return true;
                ambiguous = dialog != IntPtr.Zero;
                dialog = window;
                return !ambiguous;
            }, IntPtr.Zero);
            if (ambiguous)
                throw new InvalidOperationException($"Multiple visible dialogs titled '{title}' belong to app PID {pid}.");
            return dialog;
        }

        private static bool IsFileNameControl(IntPtr window, IntPtr dialog) {
            if (!HasClass(window, "Edit")) return false;
            int id = GetDlgCtrlID(window);
            if (id == FileNameEdit || id == FileNameCombo) return true;
            if (id != ComboEdit) return false;
            for (IntPtr parent = GetParent(window); parent != IntPtr.Zero && parent != dialog; parent = GetParent(parent)) {
                if (GetDlgCtrlID(parent) == FileNameCombo) return true;
            }
            return false;
        }

        public static IntPtr FindFileNameEdit(IntPtr dialog, uint pid) {
            IntPtr edit = IntPtr.Zero;
            bool ambiguous = false;
            EnumChildWindows(dialog, (window, data) => {
                if (!BelongsToProcess(window, pid) || !IsWindowVisible(window) || !IsWindowEnabled(window) ||
                    !IsFileNameControl(window, dialog)) return true;
                ambiguous = edit != IntPtr.Zero;
                edit = window;
                return !ambiguous;
            }, IntPtr.Zero);
            if (ambiguous)
                throw new InvalidOperationException($"Multiple File name edit controls were found in the chooser for app PID {pid}.");
            return edit;
        }

        public static IntPtr FindOpenButton(IntPtr dialog, uint pid) {
            IntPtr button = GetDlgItem(dialog, IdOk);
            return BelongsToProcess(button, pid) && IsWindowVisible(button) && HasClass(button, "Button")
                ? button : IntPtr.Zero;
        }

        public static void Submit(IntPtr dialog, IntPtr edit, IntPtr button, uint pid, string title, string path) {
            if (!MatchesDialog(dialog, pid, title) || !BelongsToProcess(edit, pid) || !IsChild(dialog, edit) ||
                !IsFileNameControl(edit, dialog) || !IsWindowVisible(edit) || !IsWindowEnabled(edit))
                throw new InvalidOperationException("The archive chooser or its File name edit control changed before entry.");

            if (SetTextMessage(edit, WmSetText, UIntPtr.Zero, path, AbortIfHung, MessageTimeoutMs,
                    out UIntPtr result) == IntPtr.Zero)
                throw new Win32Exception(Marshal.GetLastWin32Error(), "WM_SETTEXT failed or timed out on the File name edit control.");
            if (result == UIntPtr.Zero)
                throw new InvalidOperationException("The File name edit control rejected the archive path.");

            var entered = new StringBuilder(path.Length + 2);
            if (GetTextMessage(edit, WmGetText, (UIntPtr)entered.Capacity, entered, AbortIfHung, MessageTimeoutMs,
                    out _) == IntPtr.Zero)
                throw new Win32Exception(Marshal.GetLastWin32Error(), "WM_GETTEXT failed or timed out on the File name edit control.");
            if (!string.Equals(entered.ToString(), path, StringComparison.Ordinal))
                throw new InvalidOperationException("The File name edit control did not retain the full archive path.");

            if (!MatchesDialog(dialog, pid, title) || FindOpenButton(dialog, pid) != button ||
                !IsWindowEnabled(dialog) || !IsWindowEnabled(button))
                throw new InvalidOperationException("The archive chooser or its Open button changed or became disabled before submission.");

            // BN_CLICKED is zero in the high word. Queue the click so a validation prompt cannot block the caller.
            if (!PostMessageW(dialog, WmCommand, (IntPtr)IdOk, button))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Could not submit the archive chooser's Open command.");
        }
    }
}
'@

$title = '选择本地 Mod 归档'
$wait = [Diagnostics.Stopwatch]::StartNew()
$observedDialog = $false
while ($wait.ElapsedMilliseconds -lt 10000) {
    if ($app.HasExited) {
        throw "App PID $AppPid exited while waiting for '$title'."
    }
    $dialog = [AmmSmoke.ArchiveChooser]::FindDialog($AppPid, $title)
    if ($dialog -ne [IntPtr]::Zero) {
        $observedDialog = $true
        $edit = [AmmSmoke.ArchiveChooser]::FindFileNameEdit($dialog, $AppPid)
        $button = [AmmSmoke.ArchiveChooser]::FindOpenButton($dialog, $AppPid)
        if ($edit -ne [IntPtr]::Zero -and $button -ne [IntPtr]::Zero -and $wait.ElapsedMilliseconds -lt 10000) {
            [AmmSmoke.ArchiveChooser]::Submit($dialog, $edit, $button, $AppPid, $title, $ArchivePath)
            $wait.Restart()
            while ($wait.ElapsedMilliseconds -lt 5000) {
                if ($app.HasExited) {
                    throw "App PID $AppPid exited after submitting '$ArchivePath' in '$title'."
                }
                if (-not [AmmSmoke.ArchiveChooser]::BelongsToProcess($dialog, $AppPid)) {
                    return
                }
                Start-Sleep -Milliseconds ([Math]::Min(50, [Math]::Max(0, 5000 - $wait.ElapsedMilliseconds)))
            }
            throw "'$title' in app PID $AppPid remained open 5 seconds after submitting '$ArchivePath'. Check for a file validation prompt."
        }
    }
    Start-Sleep -Milliseconds ([Math]::Min(50, [Math]::Max(0, 10000 - $wait.ElapsedMilliseconds)))
}
if ($observedDialog) {
    throw "Found '$title' in app PID $AppPid, but its File name edit and Open button were not ready within 10 seconds. Inspect the native chooser controls."
}
throw "No visible native dialog titled '$title' appeared for app PID $AppPid within 10 seconds. Check that the smoke opened the chooser on the same Windows desktop."
