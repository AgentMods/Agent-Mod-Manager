import { openUrl } from "@tauri-apps/plugin-opener";
import type { ComponentProps } from "react";
import { backendAvailable, errorText } from "@/lib/api";
import { tr } from "@/lib/i18n";
import { publicHttpsUrl } from "@/lib/sponsors";
import { useUI } from "@/lib/store";

const defaultSponsorUrl = "https://sponsor.aoe.top/";

export function SponsorLink({ url, onClick, ...props }: Omit<ComponentProps<"a">, "href"> & { url?: string | null }) {
  const notify = useUI(state => state.notify);
  const targetUrl = (url ? publicHttpsUrl(url) : null) ?? defaultSponsorUrl;
  return <a {...props} href={targetUrl} target="_blank" rel="noreferrer" onClick={event => {
    onClick?.(event);
    if (event.defaultPrevented || !backendAvailable) return;
    event.preventDefault();
    void openUrl(targetUrl).catch(error => notify(tr("无法打开赞助页面：{value1}", { value1: errorText(error) })));
  }} />;
}
