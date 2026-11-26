import { useRef, useState } from "react";
import { ChevronRightIcon, SearchIcon } from "lucide-react";
import { tr } from "@/lib/i18n";
import { Modal } from "@/components/ui";
import { Badge } from "@/components/ui/badge";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";

export type McpConnection = {
  serverId: string;
  tools: string[];
  toolDetails?: { name: string; description: string }[];
};

type Props = { name: string; connection: McpConnection };

function ToolListDialog({ name, connection, onClose, restoreFocus }: Props & { onClose: () => void; restoreFocus: () => void }) {
  const [query, setQuery] = useState("");
  const tools = connection.toolDetails ?? connection.tools.map((name) => ({ name, description: "" }));
  const filter = query.trim().toLowerCase();
  const visible = filter ? tools.filter((tool) => tool.name.toLowerCase().includes(filter)) : tools;

  return <Modal title={tr("{name} 的工具", { name })} description={tr("共 {count} 个工具", { count: tools.length })}
    className="sm:max-w-xl [overflow-wrap:anywhere]" onClose={onClose}
    onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(); }}>
    {tools.length > 0 && <InputGroup className="shrink-0">
      <InputGroupAddon><SearchIcon aria-hidden="true" /></InputGroupAddon>
      <InputGroupInput type="search" aria-label={tr("搜索工具名称")} placeholder={tr("搜索工具名称")}
        value={query} onChange={(event) => setQuery(event.target.value)} />
    </InputGroup>}
    {visible.length > 0 ? <ul className="max-h-[55dvh] min-h-0 divide-y overflow-y-auto rounded-md border">
      {visible.map(({ name, description }) => <li key={name} className="space-y-1 px-3 py-2">
        <code className="text-sm font-medium">{name}</code>
        {description && <p className="whitespace-pre-wrap text-xs leading-relaxed text-muted-foreground">{description}</p>}
      </li>)}
    </ul> : <p role="status" className="py-4 text-center text-sm text-muted-foreground">
      {tools.length ? tr("没有匹配结果") : tr("此连接没有提供工具。")}
    </p>}
  </Modal>;
}

export function McpToolListButton({ name, connection }: Props) {
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const label = tr("查看 {name} 的工具", { name });

  return <>
    <Badge asChild variant="success" className="mr-1 hover:bg-success/20">
      <button type="button" ref={trigger} aria-haspopup="dialog" aria-label={label} title={label} onClick={() => setOpen(true)}>
        {tr("运行中")}<span className="hidden sm:inline">{tr("· {length} 个工具", { length: connection.tools.length })}</span>
        <ChevronRightIcon aria-hidden="true" />
      </button>
    </Badge>
    {open && <ToolListDialog name={name} connection={connection} onClose={() => setOpen(false)} restoreFocus={() => trigger.current?.focus()} />}
  </>;
}
