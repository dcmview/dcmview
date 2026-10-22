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

/** The id of `LabelSchema::implicit`, the schema a session without a document's has. */
export const IMPLICIT_SCHEMA_ID = "implicit";

/**
 * Whether a file's native read shows anything the EMBED view does not. It
 * does not when it has no label and its records are only rectangles the
 * EMBED view shows, under the implicit schema and in layers without a color:
 * the records of an EMBED CSV and of rectangles drawn in the viewer. The
 * viewer then draws and lists the rectangles exactly as without the native
 * read.
 */
export function showsMoreThanEmbed(response: NativeAnnotationsResponse): boolean {
	if (response.labels.length > 0) return true;
	if (response.annotations.length === 0) return false;
	if (response.schema.schema_id !== IMPLICIT_SCHEMA_ID) return true;
	const colored = new Set(response.layers.filter((layer) => layer.color).map((layer) => layer.id));
	return response.annotations.some((record) => record.embed_index === null || colored.has(record.layer));
}
