import * as React from "react"
import { ChevronRightIcon } from "lucide-react"
import { cn } from "@/lib/utils"
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible"

function Trace({ className, ...props }: React.ComponentProps<"div">) {
  return <div data-slot="trace" className={cn("flex w-full min-w-0 flex-col gap-2", className)} {...props} />
}

function TraceLine({ className, status, ...props }: React.ComponentProps<"span"> & { status?: "running" | "error" | "pending" }) {
  return <span data-slot="trace-line" data-status={status} className={cn("flex w-fit max-w-full min-w-0 items-center gap-2 text-sm text-muted-foreground transition-colors data-[status=error]:text-destructive data-[status=pending]:opacity-60 data-[status=running]:*:data-[slot=trace-title]:animate-pulse", className)} {...props} />
}
function TraceIcon({ className, ...props }: React.ComponentProps<"span">) {
  return <span data-slot="trace-icon" aria-hidden="true" className={cn("flex size-4 shrink-0 items-center justify-center [&_svg]:pointer-events-none [&_svg:not([class*='size-'])]:size-4", className)} {...props} />
}
function TraceTitle({ className, ...props }: React.ComponentProps<"span">) { return <span data-slot="trace-title" className={cn("min-w-0 truncate", className)} {...props} /> }
function TraceDescription({ className, ...props }: React.ComponentProps<"span">) { return <span data-slot="trace-description" className={cn("shrink-0 text-muted-foreground/70 tabular-nums", className)} {...props} /> }
function TraceCollapsible({ className, ...props }: React.ComponentProps<typeof Collapsible>) { return <Collapsible data-slot="trace-collapsible" className={cn("flex w-full min-w-0 flex-col", className)} {...props} /> }
function TraceCollapsibleTrigger({ className, children, ...props }: React.ComponentProps<typeof CollapsibleTrigger>) {
  return <CollapsibleTrigger data-slot="trace-collapsible-trigger" className={cn("group/trace-collapsible-trigger flex w-full max-w-full min-w-0 cursor-pointer items-center gap-1 rounded-lg px-2 py-1 text-left text-muted-foreground transition-colors outline-none hover:bg-muted/70 hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/30", className)} {...props}>{children}<TraceIcon className="ms-auto transition-transform group-data-[state=open]/trace-collapsible-trigger:rotate-90"><ChevronRightIcon /></TraceIcon></CollapsibleTrigger>
}
function TraceCollapsibleContent({ className, children, ...props }: React.ComponentProps<typeof CollapsibleContent>) {
  return <CollapsibleContent data-slot="trace-collapsible-content" className={cn("min-w-0 overflow-hidden duration-200 data-[state=closed]:animate-accordion-up data-[state=open]:animate-accordion-down", className)} {...props}><div data-slot="trace-collapsible-content-inner" className="mt-1 mb-1 ml-2 min-w-0 border-l pr-1 pl-4 leading-relaxed wrap-break-word text-muted-foreground">{children}</div></CollapsibleContent>
}
export { Trace, TraceLine, TraceIcon, TraceTitle, TraceDescription, TraceCollapsible, TraceCollapsibleTrigger, TraceCollapsibleContent }
