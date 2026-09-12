/**
 * `POST /api/media/upload-url`. `shared::responses::media::UploadUrlResponse`.
 *
 * `url` is the whole loadable URL, and it is what a create/edit request must carry —
 * `shared::media::is_media_url` validates the origin and the path shape together, so a
 * bare bucket key is rejected.
 *
 * It used to be `key`, and the rename was silent in the worst way: reading a `key` that
 * no longer existed yielded `undefined`, and `JSON.stringify` renders `undefined` inside
 * an array as `null` — so a create request went out with `images: [null]` and failed
 * validation with nothing in the console.
 */
export type UploadUrlResponse = {
  url: string;
  uploadUrl: string;
};
