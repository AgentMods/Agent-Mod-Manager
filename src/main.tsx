import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "react-router/dom";
import { pendingOnboardingMigration } from "./lib/store";
import { currentRoute, router } from "./router";
import { initializeLanguage } from "./lib/app-language";
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import "./styles.css";

// A desktop restart during migration offers “Continue setup” on Home. Ordinary reloads keep their URL.
if (currentRoute().view === "onboarding" && pendingOnboardingMigration()) {
  void router.navigate("/", { replace: true });
}

const queryClient = new QueryClient({defaultOptions: {queries: {staleTime: 10_000, retry: false, refetchOnWindowFocus: false}}});
const root = ReactDOM.createRoot(document.getElementById("root")!);
void initializeLanguage().then(() => root.render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <TooltipProvider delayDuration={300}><RouterProvider router={router} /></TooltipProvider>
      <Toaster position="bottom-center"/>
    </QueryClientProvider>
  </React.StrictMode>,
));