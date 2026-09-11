/** `POST /api/media/upload-url`. `shared::requests::media::UploadUrlRequest`. */
export type UploadUrlRequest = {
  kind: MediaKind;
  contentType: string;
  contentLength: number;
};

/** Decides the bucket prefix, and with it which `is_media_url` check the URL must pass. */
export type MediaKind = "spot" | "avatar";
