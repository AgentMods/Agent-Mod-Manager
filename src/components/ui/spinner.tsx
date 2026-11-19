import { cn } from "@/lib/utils"
import { Loader2Icon } from "lucide-react"

// Decorative on purpose: the surrounding text (or a `role="status"` wrapper) announces progress.
// A built-in role="status" here would add a second live region next to every busy button.
function Spinner({ className, ...props }: React.ComponentProps<"svg">) {
  return (
    <Loader2Icon
      aria-hidden="true"
      className={cn("size-4 animate-spin", className)}
      {...props}
    />
  )
}

export { Spinner }