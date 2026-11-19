import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import App from "./App";
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import "./styles.css";

const queryClient = new QueryClient({defaultOptions: {queries: {staleTime: 10_000, retry: false, refetchOnWindowFocus: false}}});
ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <TooltipProvider delayDuration={300}><App/></TooltipProvider>
      <Toaster position="bottom-center" containerAriaLabel="通知"/>
    </QueryClientProvider>
  </React.StrictMode>,
);