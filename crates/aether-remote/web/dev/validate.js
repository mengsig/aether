import { readFileSync } from 'node:fs';
import { Ajv2020 } from 'ajv/dist/2020.js';
const schema = JSON.parse(readFileSync(new URL('./protocol.schema.json',import.meta.url),'utf8'));
const ajv = new Ajv2020({strict:false,validateFormats:false,allErrors:false});
ajv.addSchema(schema,'aether');
/** @type {Map<string,import('ajv').ValidateFunction>} */ const validators = new Map();
/** @param {string} name @param {unknown} value */
export function assertProtocol(name, value) {
  if (!Object.hasOwn(schema.$defs,name)) throw new Error(`Unknown schema type ${name}`);
  const reference = schemaReference(name,value);
  let validate = validators.get(reference);
  if (!validate) { validate = ajv.compile({$ref:reference}); validators.set(reference,validate); }
  if (!validate(value)) throw new Error(`Invalid ${name}: ${ajv.errorsText(validate.errors?.slice(0,8),{separator:'; '})}`);
}
/** @param {string} name @param {unknown} value */
function schemaReference(name,value) {
  const reference = `aether#/$defs/${name}`;
  if (name !== 'Reply' || !value || typeof value !== 'object' || !('reply' in value)) return reference;
  const variants = schema.$defs.Reply.oneOf;
  if (!Array.isArray(variants)) throw new Error('Reply schema must use a tagged union');
  const index = variants.findIndex(variant => variant.properties?.reply?.const === value.reply);
  if (index < 0) throw new Error(`Invalid Reply tag: ${String(value.reply)}`);
  return `${reference}/oneOf/${index}`;
}
/** @param {Record<string,unknown>} value */
export function validateCommand(value) {
  if (String(value.cmd).startsWith('mock_')) return;
  const input = {...value}; delete input.mock_drop_reply;
  assertProtocol('ClientFrame',input);
}
