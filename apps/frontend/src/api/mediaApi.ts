import { useMutation } from "@tanstack/vue-query";

import { ApiError, post } from "./client";
import type { MediaKind } from "@/types/requests/media/UploadUrlRequest";
import type { UploadUrlResponse } from "@/types/responses/media/UploadUrlResponse";

/**
 * Uploads one image straight to R2 and returns the URL to store.
 *
 * Two requests, and the second one does not go through the api client: it is a
 * plain PUT to Cloudflare against a presigned URL, so it must not carry our
 * Authorization header or the SameSite cookie — the signature is the whole auth,
 * and an extra signed-header mismatch would just 403.
 *
 * `Content-Type` and `Content-Length` are signed into that URL by media-service,
 * so they have to match what was declared or R2 rejects the upload. That is also
 * what enforces the size cap.
 */
export async function uploadImage(file: File, kind: MediaKind): Promise<string> {
  const { url, uploadUrl } = await post<UploadUrlResponse>(
    "/api/media/upload-url",
    { kind, contentType: file.type, contentLength: file.size },
  );

  const uploaded = await fetch(uploadUrl, {
    method: "PUT",
    headers: { "Content-Type": file.type },
    body: file,
  });

  // R2's failures are not `MyError` bodies, so this is the one place that mints an
  // `ApiError` by hand rather than parsing one. It is still an `ApiError`, so a
  // caller catching the mint above and the upload here has one type to handle.
  if (!uploaded.ok) {
    throw new ApiError(
      uploaded.status,
      "Upload failed",
      [`Upload failed (${uploaded.status}). Please try again.`],
      {},
    );
  }

  return url;
}

/**
 * Resolves a picker's mixed list to URLs, uploading only what is new.
 *
 * The spot forms hold `(string | File)[]` — strings are URLs already in R2 from a
 * previous save, Files are freshly picked. Order is the host's, so it is
 * preserved rather than uploaded-last.
 */
export function uploadNewImages(
  images: (string | File)[],
  kind: MediaKind,
): Promise<string[]> {
  return Promise.all(
    images.map((image) =>
      typeof image === "string" ? image : uploadImage(image, kind),
    ),
  );
}

/**
 * No cache to invalidate — an upload writes to R2, not to a projection. This exists
 * for `isPending` and a typed `error`, which the two spot forms were tracking with
 * refs of their own.
 */
export const useUploadImages = () =>
  useMutation({
    mutationFn: ({
      images,
      kind,
    }: {
      images: (string | File)[];
      kind: MediaKind;
    }) => uploadNewImages(images, kind),
  });
