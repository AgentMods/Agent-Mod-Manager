const verbatimPrefix = "\\\\?\\";
const windowsDrive = /^[A-Za-z]:[\\/]/;
const dosDevice = /^(?:CON|PRN|AUX|NUL|CONIN\$|CONOUT\$|CLOCK\$|(?:COM|LPT)[1-9¹²³])$/;

function ordinaryWindowsComponents(tail: string) {
  return tail.replace(/\\$/, "").split("\\").every((name) =>
    name !== "" && !/[\/<>:"|?*\x00-\x1f\x7f-\x9f]/.test(name) && !/[. ]$/.test(name) &&
    !dosDevice.test(name.split(".", 1)[0].replace(/ +$/, "").toUpperCase()),
  );
}

/** Presentation only. Match Rust's user_path for legacy data carrying canonical Windows paths. */
export function displayPath(path: string): string {
  if (!path.startsWith(verbatimPrefix)) return path;
  const body = path.slice(verbatimPrefix.length);
  if (/^UNC\\/i.test(body)) {
    const tail = body.slice(4);
    if (/^[^\\]+\\[^\\]/.test(tail) && ordinaryWindowsComponents(tail)) return "\\\\" + tail;
  } else if (/^[A-Za-z]:\\/.test(body)) {
    const tail = body.slice(3);
    if (!tail || ordinaryWindowsComponents(tail)) return body;
  }
  return path;
}

/** Conservative directory identity for UI reconciliation only; native code resolves real aliases. */
export function gameDirectoryKey(path: string): string {
  const display = displayPath(path);
  // Do not fold filename case, particularly for UNC servers and Unix filesystems.
  // Dot components can cross a symlink or be literal in verbatim paths.
  if (windowsDrive.test(display)) {
    const rest = display.slice(3).replaceAll("/", "\\");
    if (!rest || ordinaryWindowsComponents(rest)) return `${display[0].toUpperCase()}:\\${rest.replace(/\\$/, "")}`;
  } else if (/^\\\\[^\\/?]+\\[^\\]/.test(display) && ordinaryWindowsComponents(display.slice(2))) {
    return display.replace(/\\$/, "");
  }
  return display;
}

function pathSeparator(path: string): "/" | "\\" {
  return windowsDrive.test(path) || path.startsWith("\\\\") ? "\\" : "/";
}

function windowsNamespace(path: string) {
  return path.startsWith(verbatimPrefix) || path.startsWith("\\\\.\\");
}

/** For an absolute filesystem path, not arbitrary text or an archive entry. */
export function pathBasename(path: string): string {
  const separator = pathSeparator(path);
  return path.split(separator === "\\" && !windowsNamespace(path) ? /[\\/]/ : separator).at(-1) ?? "";
}

/** Append a fixed leaf to an absolute directory picked by the OS, without normalizing the parent. */
export function appendPathLeaf(parent: string, leaf: string): string {
  const separator = pathSeparator(parent);
  const hasSeparator = parent.endsWith(separator) ||
    (separator === "\\" && !windowsNamespace(parent) && parent.endsWith("/"));
  return `${parent}${hasSeparator ? "" : separator}${leaf}`;
}
