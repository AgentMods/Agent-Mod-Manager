import { queryOptions, useQuery } from "@tanstack/react-query";
import { api, backendAvailable } from "./api";

export type DashboardStats = {
  managedGameCount: number;
  installedModCount: number;
  totalModCount: number;
};

export const dashboardStatsKey = ["dashboard-stats"] as const;

export const dashboardStatsQuery = queryOptions({
  queryKey: dashboardStatsKey,
  queryFn: () => api<DashboardStats>("get_dashboard_stats"),
  enabled: backendAvailable,
  retry: false,
  staleTime: 0,
});

export function useDashboardStats() {
  return useQuery(dashboardStatsQuery);
}
