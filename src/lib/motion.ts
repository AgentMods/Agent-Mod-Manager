import { useEffect, useRef, useState, type CSSProperties } from "react";

/** Entrance delay for the nth item of a list. The cap keeps a long list from making the last row wait. */
export function stagger(index: number, step = 45, max = 8): CSSProperties {
  return { "--stagger": `${Math.min(Math.max(index, 0), max) * step}ms` } as CSSProperties;
}

/**
 * True once `value` differs from the value it had on first render. Use it to play a one-off effect when
 * something changes (a task finishing) without replaying it for rows that mount already in that state.
 */
export function useDidChange<T>(value: T) {
  const initial = useRef(value);
  const [changed, setChanged] = useState(false);
  useEffect(() => { if (value !== initial.current) setChanged(true); }, [value]);
  return changed;
}
