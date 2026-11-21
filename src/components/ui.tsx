import type { ComponentProps, ReactNode } from "react";
import { AlertTriangleIcon, InboxIcon } from "lucide-react";
import { errorText } from "@/lib/api";
import { cn } from "@/lib/utils";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge as StatusBadge } from "@/components/ui/badge";
import { Button as UiButton } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

/** Page frame shared by every top-level view: width cap, padding and vertical rhythm. */
export function Page({ className, ...props }: ComponentProps<"div">) {
  return <div className={cn("mx-auto flex min-h-full w-full max-w-[1760px] flex-col gap-6 p-4 motion-safe:animate-page-in sm:p-6 lg:p-8", className)} {...props}/>;
}

export function PageHeader({ title, subtitle, actions }: { title: string; subtitle?: string; actions?: ReactNode }) {
  return <header className="flex flex-wrap items-center justify-between gap-4">
    <div className="flex min-w-0 flex-col gap-1">
      <h1 className="text-2xl font-semibold tracking-tight">{title}</h1>
      {subtitle && <p className="text-sm text-muted-foreground">{subtitle}</p>}
    </div>
    {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
  </header>;
}

/**
 * TEMPORARY migration shim: pages still importing `Button` from this module keep working while
 * they are moved to `@/components/ui/button`. Delete once `grep "Button.*from .*components/ui\"` is empty.
 */
export function Button({ className, ...props }: ComponentProps<typeof UiButton>) {
  return <UiButton variant="outline" className={cn("btn", className)} {...props}/>;
}

/** Icon-only button. The label is both the accessible name and the tooltip. */
export function IconButton({ title, children, variant = "ghost", size = "icon-sm", ...props }: ComponentProps<typeof Button> & { title: string }) {
  return <Tooltip>
    <TooltipTrigger asChild><UiButton variant={variant} size={size} aria-label={title} {...props}>{children}</UiButton></TooltipTrigger>
    <TooltipContent>{title}</TooltipContent>
  </Tooltip>;
}

export function EmptyState({ title, children, action, icon }: { title: string; children?: ReactNode; action?: ReactNode; icon?: ReactNode }) {
  return <Empty className="border motion-safe:animate-rise-in">
    <EmptyHeader>
      <EmptyMedia variant="icon" className="motion-safe:animate-float">{icon ?? <InboxIcon/>}</EmptyMedia>
      <EmptyTitle>{title}</EmptyTitle>
      {children && <EmptyDescription>{children}</EmptyDescription>}
    </EmptyHeader>
    {action && <EmptyContent>{action}</EmptyContent>}
  </Empty>;
}

export function ErrorState({ error, className }: { error: unknown; className?: string }) {
  return error ? <Alert variant="destructive" className={cn("motion-safe:animate-rise-in", className)}><AlertTriangleIcon/><AlertDescription className="break-words">{errorText(error)}</AlertDescription></Alert> : null;
}

const toneVariants = { success: "success", warning: "warning", error: "danger", danger: "danger", neutral: "secondary" } as const;
/** Status chip. `tone` is the legacy vocabulary used by the data layer. */
export function Badge({ children, tone, className }: { children: ReactNode; tone?: string; className?: string }) {
  const variant = tone && tone in toneVariants ? toneVariants[tone as keyof typeof toneVariants] : "secondary";
  return <StatusBadge variant={variant} className={cn("h-auto min-h-5 whitespace-normal", className)}>{children}</StatusBadge>;
}

/** File path or other machine text. Wraps instead of overflowing. */
export function PathText({ className, ...props }: ComponentProps<"span">) {
  return <span className={cn("font-mono text-xs break-all text-muted-foreground", className)} {...props}/>;
}

export function bytes(value: number) {
  if (!Number.isFinite(value) || value < 0) return "-";
  const units = ["B", "KiB", "MiB", "GiB"];
  let index = 0;
  while (value >= 1024 && index < units.length - 1) { value /= 1024; index++; }
  return `${value.toFixed(index ? 1 : 0)} ${units[index]}`;
}
export function date(value: string) {
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? value : parsed.toLocaleString("zh-CN", { dateStyle: "short", timeStyle: "short" });
}

/**
 * Modal task dialog. Closes with Escape, the close button or an explicit action,
 * never by clicking outside, so a stray click cannot reject an approval.
 */
export function Modal({ title, description, children, actions, onClose, onCloseAutoFocus, className }: {
  title: string; description?: string; children: ReactNode; actions?: ReactNode; onClose: () => void; className?: string;
  onCloseAutoFocus?: ComponentProps<typeof DialogContent>["onCloseAutoFocus"];
}) {
  return <Dialog open onOpenChange={(open) => { if (!open) onClose(); }}>
    <DialogContent onInteractOutside={(event) => event.preventDefault()} onCloseAutoFocus={onCloseAutoFocus}
      className={cn("flex max-h-[calc(100dvh-3rem)] flex-col gap-0 p-0 sm:max-w-[850px]", className)}>
      <DialogHeader className="border-b px-6 py-4 pr-12 text-left">
        <DialogTitle>{title}</DialogTitle>
        {description ? <DialogDescription>{description}</DialogDescription> : <DialogDescription className="sr-only">{title}</DialogDescription>}
      </DialogHeader>
      <div data-slot="modal-body" className="flex min-h-0 flex-1 flex-col gap-4 overflow-auto px-6 py-5">{children}</div>
      {actions && <DialogFooter className="border-t px-6 py-4">{actions}</DialogFooter>}
    </DialogContent>
  </Dialog>;
}