import { api } from "./api";

/** Opaque capabilities issued by the native browse session; never include links or provider IDs. */
export interface BrowseInstallWork {
  searchId: string;
  itemId: string;
}

export type BrowseInstallStatus = "running" | "installed" | "unverified" | "rejected" | "cancelled" | "interrupted" | "failed";

export interface BrowseInstallTask {
  id: string;
  conversationId: string;
  turnId: string | null;
  status: BrowseInstallStatus;
  installId: string | null;
  transactionId: string | null;
}

export function browseInstallKey(work: BrowseInstallWork, gameId: string) {
  return ["browse-install", work.searchId, work.itemId, gameId] as const;
}

export function readBrowseInstall(work: BrowseInstallWork, gameId: string) {
  return api<BrowseInstallTask | null>("browse_install_task", { searchId: work.searchId, itemId: work.itemId, gameId });
}

export function startBrowseInstall(work: BrowseInstallWork, gameId: string) {
  return api<BrowseInstallTask>("start_browse_install", { searchId: work.searchId, itemId: work.itemId, gameId });
}

export async function cancelBrowseInstall({ conversationId, turnId, status }: BrowseInstallTask) {
  if (status === "running" && turnId) await api("cancel_turn", { conversationId, turnId });
}

export function browseInstallStatus(task: BrowseInstallTask): BrowseInstallStatus {
  // A terminal Agent or download event is never a receipt. Only native task
  // reconciliation may assert success, with both linked evidence identifiers.
  return task.status === "installed" && (!task.installId || !task.transactionId) ? "unverified" : task.status;
}
