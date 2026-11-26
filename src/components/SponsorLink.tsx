import { openUrl } from "@tauri-apps/plugin-opener";
import type { ComponentProps } from "react";
import { backendAvailable, errorText } from "@/lib/api";
import { tr } from "@/lib/i18n";
import { useUI } from "@/lib/store";

const sponsorUrl = "https://sponsor.aoe.top/";

export function SponsorLink({ onClick, ...props }: ComponentProps<"a">) {
  const notify = useUI(state => state.notify);
  return <a {...props} href={sponsorUrl} target="_blank" rel="noreferrer" onClick={event => {
    onClick?.(event);
    if (event.defaultPrevented || !backendAvailable) return;
    event.preventDefault();
    void openUrl(sponsorUrl).catch(error => notify(tr("无法打开赞助页面：{value1}", { value1: errorText(error) })));
  }} />;
}
