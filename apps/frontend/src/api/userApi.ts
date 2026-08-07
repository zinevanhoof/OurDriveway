import { apiFetch } from "./king";
import { LoginRequest } from "@/types/requests/LoginRequest";
import { SignupRequest } from "@/types/requests/SignupRequest";

const loginUser = async ({
  email,
  password,
}: LoginRequest): Promise<Response> => {
  const response = await apiFetch("/api/user/login", {
    method: "POST",
    body: JSON.stringify({
      email,
      password,
    }),
    headers: {
      "Content-Type": "application/json",
    },
  });

  return response;
};

const signupUser = async ({
  firstName,
  lastName,
  email,
  password,
}: SignupRequest): Promise<Response> => {
  const response = await apiFetch("/api/user/signup", {
    method: "POST",
    body: JSON.stringify({
      firstName,
      lastName,
      email,
      password,
    }),
    headers: {
      "Content-Type": "application/json",
    },
  });

  return response;
};

const logoutUser = async () => {
  await apiFetch("/api/user/refresh/logout", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
  });
};

const refreshUser = async (): Promise<Response> => {
  const response = await apiFetch("/api/user/refresh", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
  });

  return response;
};

export { loginUser, signupUser, logoutUser, refreshUser };
