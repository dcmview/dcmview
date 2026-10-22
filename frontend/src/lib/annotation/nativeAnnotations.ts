import type { Annotation, Label, LabelSchema, Layer } from "../../generated/annotation-types";

/**
 * One record of a file's native annotations, with its position in the EMBED
 * view of the same file: the index the rectangle list and its editing use for
 * a rectangle that view shows, else `null`.
 */
export type NativeAnnotationRecord = Annotation & { embed_index: number | null };

/**
 * `GET /api/file/{index}/annotations/native`: everything the viewer draws and
 * lists for one file. `schema` is the session schema, or the implicit one when
 * the document has none, so every class has a name. `layers` are the session's
 * layers, `labels` those whose target is the file, one of its frames, or its
 * series, study or patient, and `revision` the store revision the copy was
 * read at.
 *
 * Defined here until the HTTP contract generates it; the display modules take
 * this type and nothing else from the response.
 */
export type NativeAnnotationsResponse = {
	schema: LabelSchema;
	layers: Layer[];
	annotations: NativeAnnotationRecord[];
	labels: Label[];
	revision: number;
};
