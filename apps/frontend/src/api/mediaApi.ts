import { apiFetch } from "./king";
import { readErrorDetail } from "@/lib/serverErrors";

type Kind = "spot" | "avatar";

interface UploadUrlResponse {
  key: string;
  uploadUrl: string;
}

/**
 * Uploads one image straight to R2 and returns the key to store.
 *
 * Two requests, and the second one does not go through `apiFetch`: it is a plain
 * PUT to Cloudflare against a presigned URL, so it must not carry our
 * Authorization header or the SameSite cookie — the signature is the whole auth,
 * and an extra signed-header mismatch would just 403.
 *
 * `Content-Type` and `Content-Length` are signed into that URL by media-service,
 * so they have to match what was declared or R2 rejects the upload. That is also
 * what enforces the size cap.
 */
export async function uploadImage(file: File, kind: Kind): Promise<string> {
  const minted = await apiFetch("/api/media/upload-url", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      kind,
      contentType: file.type,
      contentLength: file.size,
    }),
  });

  if (!minted.ok) {
    throw new Error((await readErrorDetail(minted)).join(" "));
  }

  const { key, uploadUrl }: UploadUrlResponse = await minted.json();

  const uploaded = await fetch(uploadUrl, {
    method: "PUT",
    headers: { "Content-Type": file.type },
    body: file,
  });

  if (!uploaded.ok) {
    throw new Error(`Upload failed (${uploaded.status}). Please try again.`);
  }

  return key;
}

/**
 * Resolves a picker's mixed list to keys, uploading only what is new.
 *
 * The spot forms hold `(string | File)[]` — strings are keys already in R2 from a
 * previous save, Files are freshly picked. Order is the host's, so it is
 * preserved rather than uploaded-last.
 */
export function uploadNewImages(
  images: (string | File)[],
  kind: Kind,
): Promise<string[]> {
  return Promise.all(
    images.map((image) =>
      typeof image === "string" ? image : uploadImage(image, kind),
    ),
  );
}
