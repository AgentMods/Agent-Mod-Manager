import lightLogo from "../../assets/branding/glosc-ga.png";
import darkLogo from "../../assets/branding/glosc-ga-white.png";
import { useUI } from "@/lib/store";

/** Symmetric transparent padding allows scaling without changing the layout box. */
export function AppLogo() {
  const theme = useUI((state) => state.theme);
  return <img
    src={theme === "dark" ? darkLogo : lightLogo}
    alt=""
    width={32}
    height={32}
    className="pointer-events-none size-8 shrink-0 scale-150"
  />;
}
