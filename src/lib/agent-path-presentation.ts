import { displayPath } from "./paths";

function record(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : null;
}

type Projection = (value: unknown) => unknown;
const path: Projection = (value) => typeof value === "string" ? displayPath(value) : value;
const array = (project: Projection): Projection => (value) => Array.isArray(value) ? value.map(project) : value;

function fields(value: unknown, schema: Record<string, Projection>): unknown {
  const input = record(value);
  if (!input) return value;
  const output = { ...input };
  for (const [key, project] of Object.entries(schema)) {
    if (Object.hasOwn(input, key)) output[key] = project(input[key]);
  }
  return output;
}

function fileOperation(value: unknown) {
  switch (record(value)?.op) {
    case "copy": case "move": case "compress": case "extract": return fields(value, { src: path, dst: path });
    case "mkdir": case "write_text": return fields(value, { dst: path });
    default: return value;
  }
}

const filePlan: Projection = (value) => fields(value, { entries: array((item) => fields(item, { operation: fileOperation })) });
const installPreview: Projection = (value) => fields(value, { installRoot: path, plan: filePlan });
const fileEntries = array((item) => fields(item, { path }));
const logReport: Projection = (value) => fields(value, { files: array((file) => fields(file, { file: path })) });
const draft: Projection = (value) => fields(value, { path });
const deployment: Projection = (value) => fields(value, { installPath: path, plan: filePlan });

// The Agent payload can contain arbitrary user text or third-party MCP output.
// Project only known native schema fields, for display; do not rewrite replay data.
export function displayToolArguments(name: unknown, value: unknown): unknown {
  switch (name) {
    case "read_dir": case "read_file": case "extract_preview": case "preview_fomod":
      return fields(value, { path });
    case "plan_file_ops": return fields(value, { operations: array(fileOperation) });
    case "trash_paths": return fields(value, { paths: array(path) });
    case "plan_install": case "install_mod":
      return fields(value, { request: (request) => fields(request, { artifactPath: path }) });
    case "scan_games":
      return fields(value, { options: (options) => fields(options, {
        userRoots: array(path), steamRoots: array(path), epicRoots: array(path),
        epicManifestDirs: array(path), legendaryInstalledFiles: array(path),
      }) });
    default: return value;
  }
}

const directPlanTools = new Set<unknown>(["apply_file_ops", "trash_paths", "restore_trash", "snapshot_create", "uninstall_mod"]);

export function isFilePlanApproval(name: unknown): boolean {
  return directPlanTools.has(name) || name === "install_library_mod" || name === "install_mod" || name === "write_plugin_draft" || name === "install_plugin";
}

export function displayApprovalPreview(name: unknown, value: unknown): unknown {
  if (directPlanTools.has(name)) return filePlan(value);
  switch (name) {
    case "install_library_mod": case "install_mod": return installPreview(value);
    case "write_plugin_draft": return fields(value, { draft, plan: filePlan });
    case "install_plugin": return fields(value, { deployment });
    case "rollback_transaction": return fields(value, { originalPlan: filePlan });
    default: return value;
  }
}

export function displayToolResult(name: unknown, value: unknown): unknown {
  if (directPlanTools.has(name) || name === "plan_file_ops") {
    return fields(filePlan(value), { plan: filePlan, preview: (preview) => displayApprovalPreview(name, preview) });
  }
  switch (name) {
    case "read_dir": return fileEntries(value);
    case "download_mod": case "download_web_mod": return fields(value, { path });
    case "list_installs": return array((install) => fields(install, { installRoot: path, files: fileEntries }))(value);
    case "inspect_install": return fields(value, { files: fileEntries });
    case "read_logs": return logReport(value);
    case "diagnose_game": return fields(value, {
      logs: logReport, evidence: array((item) => fields(item, { file: path })),
      fabricStartup: (item) => fields(item, { file: path }), fabricDisk: (item) => fields(item, { directory: path }),
      repairSuggestions: array((item) => fields(item, {
        fileOpsInput: (input) => fields(input, { ops: array(fileOperation) }),
      })),
    });
    case "preview_fomod": return fields(value, { artifactPath: path });
    case "plan_install": return installPreview(value);
    case "install_library_mod": case "install_mod": case "write_plugin_draft": case "install_plugin":
      return fields(value, { plan: filePlan, preview: (preview) => displayApprovalPreview(name, preview) });
    case "list_plugin_drafts": return array(draft)(value);
    case "validate_plugin": return fields(value, { draft });
    case "scan_games": return fields(value, {
      candidates: array((candidate) => fields(candidate, {
        rootPath: path, sources: array((source) => fields(source, { metadataPath: path })),
      })),
      warnings: array((warning) => fields(warning, { path })),
    });
    default: return value;
  }
}
