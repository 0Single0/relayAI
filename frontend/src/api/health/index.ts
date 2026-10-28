import { APIRequest } from "@/utils/request";

export type HealthData = {
  service: string;
  status: string;
  version: string;
};

export const getHealth = () =>
  APIRequest<HealthData>({
    url: "/health",
    method: "GET",
  });
