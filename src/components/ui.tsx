import { useEffect, useRef, type ButtonHTMLAttributes, type ReactNode } from "react";
import { AlertTriangle, Inbox, X } from "lucide-react";
import { errorText } from "../lib/api";

export function Button({ className = "", ...props }: ButtonHTMLAttributes<HTMLButtonElement>) {
  return <button type="button" className={`btn ${className}`} {...props}/>;
}
export function IconButton({ title, className = "", ...props }: ButtonHTMLAttributes<HTMLButtonElement> & {title: string}) {
  return <button type="button" title={title} aria-label={title} className={`icon-btn ${className}`} {...props}/>;
}
export function PageHeader({title, subtitle, actions}: {title: string; subtitle?: string; actions?: ReactNode}) {
  return <header className="page-header"><div><h1>{title}</h1>{subtitle && <p>{subtitle}</p>}</div><div className="page-actions">{actions}</div></header>;
}
export function EmptyState({title, children, action}: {title: string; children?: ReactNode; action?: ReactNode}) {
  return <div className="empty-state"><Inbox size={30} aria-hidden="true"/><h2>{title}</h2>{children && <p>{children}</p>}{action}</div>;
}
export function ErrorState({error}: {error: unknown}) {
  return error ? <div role="alert" className="error-state"><AlertTriangle size={18}/><span>{errorText(error)}</span></div> : null;
}
export function Badge({children, tone = "neutral"}: {children: ReactNode; tone?: string}) {
  return <span className={`status-badge ${tone}`}>{children}</span>;
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
  return Number.isNaN(parsed.getTime()) ? value : parsed.toLocaleString("zh-CN", {dateStyle: "short", timeStyle: "short"});
}
export function Modal({title, children, actions, onClose}: {title: string; children: ReactNode; actions?: ReactNode; onClose: () => void}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => { ref.current?.showModal(); return () => ref.current?.close(); }, []);
  return <dialog ref={ref} className="modal" aria-label={title} onCancel={(event) => { event.preventDefault(); onClose(); }}>
    <header className="modal-header"><h2>{title}</h2><IconButton title="关闭" onClick={onClose}><X size={18}/></IconButton></header>
    <div className="modal-body">{children}</div>{actions && <footer className="modal-actions">{actions}</footer>}
  </dialog>;
}
