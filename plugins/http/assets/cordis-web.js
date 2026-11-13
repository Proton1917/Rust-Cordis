/*!
cbor-x 1.6.0
MIT License

Copyright (c) 2020 Kris Zyp

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

*/
// node_modules/cbor-x/decode.js
var decoder;
try {
  decoder = new TextDecoder();
} catch (error) {
}
var src;
var srcEnd;
var position = 0;
var EMPTY_ARRAY = [];
var LEGACY_RECORD_INLINE_ID = 105;
var RECORD_DEFINITIONS_ID = 57342;
var RECORD_INLINE_ID = 57343;
var BUNDLED_STRINGS_ID = 57337;
var PACKED_REFERENCE_TAG_ID = 6;
var STOP_CODE = {};
var maxArraySize = 11281e4;
var maxMapSize = 1681e4;
var maxObjectSize = 1671e4;
var strings = EMPTY_ARRAY;
var stringPosition = 0;
var currentDecoder = {};
var currentStructures;
var srcString;
var srcStringStart = 0;
var srcStringEnd = 0;
var bundledStrings;
var referenceMap;
var currentExtensions = [];
var currentExtensionRanges = [];
var packedValues;
var dataView;
var restoreMapsAsObject;
var defaultOptions = {
  useRecords: false,
  mapsAsObjects: true
};
var sequentialMode = false;
var inlineObjectReadThreshold = 2;
try {
  new Function("");
} catch (error) {
  inlineObjectReadThreshold = Infinity;
}
var Decoder = class _Decoder {
  constructor(options) {
    if (options) {
      if ((options.keyMap || options._keyMap) && !options.useRecords) {
        options.useRecords = false;
        options.mapsAsObjects = true;
      }
      if (options.useRecords === false && options.mapsAsObjects === void 0)
        options.mapsAsObjects = true;
      if (options.getStructures)
        options.getShared = options.getStructures;
      if (options.getShared && !options.structures)
        (options.structures = []).uninitialized = true;
      if (options.keyMap) {
        this.mapKey = /* @__PURE__ */ new Map();
        for (let [k, v] of Object.entries(options.keyMap)) this.mapKey.set(v, k);
      }
    }
    Object.assign(this, options);
  }
  /*
  decodeKey(key) {
  	return this.keyMap
  		? Object.keys(this.keyMap)[Object.values(this.keyMap).indexOf(key)] || key
  		: key
  }
  */
  decodeKey(key) {
    return this.keyMap ? this.mapKey.get(key) || key : key;
  }
  encodeKey(key) {
    return this.keyMap && this.keyMap.hasOwnProperty(key) ? this.keyMap[key] : key;
  }
  encodeKeys(rec) {
    if (!this._keyMap) return rec;
    let map = /* @__PURE__ */ new Map();
    for (let [k, v] of Object.entries(rec)) map.set(this._keyMap.hasOwnProperty(k) ? this._keyMap[k] : k, v);
    return map;
  }
  decodeKeys(map) {
    if (!this._keyMap || map.constructor.name != "Map") return map;
    if (!this._mapKey) {
      this._mapKey = /* @__PURE__ */ new Map();
      for (let [k, v] of Object.entries(this._keyMap)) this._mapKey.set(v, k);
    }
    let res = {};
    map.forEach((v, k) => res[safeKey(this._mapKey.has(k) ? this._mapKey.get(k) : k)] = v);
    return res;
  }
  mapDecode(source, end) {
    let res = this.decode(source);
    if (this._keyMap) {
      switch (res.constructor.name) {
        case "Array":
          return res.map((r) => this.decodeKeys(r));
      }
    }
    return res;
  }
  decode(source, end) {
    if (src) {
      return saveState(() => {
        clearSource();
        return this ? this.decode(source, end) : _Decoder.prototype.decode.call(defaultOptions, source, end);
      });
    }
    srcEnd = end > -1 ? end : source.length;
    position = 0;
    stringPosition = 0;
    srcStringEnd = 0;
    srcString = null;
    strings = EMPTY_ARRAY;
    bundledStrings = null;
    src = source;
    try {
      dataView = source.dataView || (source.dataView = new DataView(source.buffer, source.byteOffset, source.byteLength));
    } catch (error) {
      src = null;
      if (source instanceof Uint8Array)
        throw error;
      throw new Error("Source must be a Uint8Array or Buffer but was a " + (source && typeof source == "object" ? source.constructor.name : typeof source));
    }
    if (this instanceof _Decoder) {
      currentDecoder = this;
      packedValues = this.sharedValues && (this.pack ? new Array(this.maxPrivatePackedValues || 16).concat(this.sharedValues) : this.sharedValues);
      if (this.structures) {
        currentStructures = this.structures;
        return checkedRead();
      } else if (!currentStructures || currentStructures.length > 0) {
        currentStructures = [];
      }
    } else {
      currentDecoder = defaultOptions;
      if (!currentStructures || currentStructures.length > 0)
        currentStructures = [];
      packedValues = null;
    }
    return checkedRead();
  }
  decodeMultiple(source, forEach) {
    let values, lastPosition = 0;
    try {
      let size = source.length;
      sequentialMode = true;
      let value = this ? this.decode(source, size) : defaultDecoder.decode(source, size);
      if (forEach) {
        if (forEach(value) === false) {
          return;
        }
        while (position < size) {
          lastPosition = position;
          if (forEach(checkedRead()) === false) {
            return;
          }
        }
      } else {
        values = [value];
        while (position < size) {
          lastPosition = position;
          values.push(checkedRead());
        }
        return values;
      }
    } catch (error) {
      error.lastPosition = lastPosition;
      error.values = values;
      throw error;
    } finally {
      sequentialMode = false;
      clearSource();
    }
  }
};
function checkedRead() {
  try {
    let result = read();
    if (bundledStrings) {
      if (position >= bundledStrings.postBundlePosition) {
        let error = new Error("Unexpected bundle position");
        error.incomplete = true;
        throw error;
      }
      position = bundledStrings.postBundlePosition;
      bundledStrings = null;
    }
    if (position == srcEnd) {
      currentStructures = null;
      src = null;
      if (referenceMap)
        referenceMap = null;
    } else if (position > srcEnd) {
      let error = new Error("Unexpected end of CBOR data");
      error.incomplete = true;
      throw error;
    } else if (!sequentialMode) {
      throw new Error("Data read, but end of buffer not reached");
    }
    return result;
  } catch (error) {
    clearSource();
    if (error instanceof RangeError || error.message.startsWith("Unexpected end of buffer")) {
      error.incomplete = true;
    }
    throw error;
  }
}
function read() {
  let token = src[position++];
  let majorType = token >> 5;
  token = token & 31;
  if (token > 23) {
    switch (token) {
      case 24:
        token = src[position++];
        break;
      case 25:
        if (majorType == 7) {
          return getFloat16();
        }
        token = dataView.getUint16(position);
        position += 2;
        break;
      case 26:
        if (majorType == 7) {
          let value = dataView.getFloat32(position);
          if (currentDecoder.useFloat32 > 2) {
            let multiplier = mult10[(src[position] & 127) << 1 | src[position + 1] >> 7];
            position += 4;
            return (multiplier * value + (value > 0 ? 0.5 : -0.5) >> 0) / multiplier;
          }
          position += 4;
          return value;
        }
        token = dataView.getUint32(position);
        position += 4;
        break;
      case 27:
        if (majorType == 7) {
          let value = dataView.getFloat64(position);
          position += 8;
          return value;
        }
        if (majorType > 1) {
          if (dataView.getUint32(position) > 0)
            throw new Error("JavaScript does not support arrays, maps, or strings with length over 4294967295");
          token = dataView.getUint32(position + 4);
        } else if (currentDecoder.int64AsNumber) {
          token = dataView.getUint32(position) * 4294967296;
          token += dataView.getUint32(position + 4);
        } else
          token = dataView.getBigUint64(position);
        position += 8;
        break;
      case 31:
        switch (majorType) {
          case 2:
          // byte string
          case 3:
            throw new Error("Indefinite length not supported for byte or text strings");
          case 4:
            let array = [];
            let value, i = 0;
            while ((value = read()) != STOP_CODE) {
              if (i >= maxArraySize) throw new Error(`Array length exceeds ${maxArraySize}`);
              array[i++] = value;
            }
            return majorType == 4 ? array : majorType == 3 ? array.join("") : Buffer.concat(array);
          case 5:
            let key;
            if (currentDecoder.mapsAsObjects) {
              let object = {};
              let i2 = 0;
              if (currentDecoder.keyMap) {
                while ((key = read()) != STOP_CODE) {
                  if (i2++ >= maxMapSize) throw new Error(`Property count exceeds ${maxMapSize}`);
                  object[safeKey(currentDecoder.decodeKey(key))] = read();
                }
              } else {
                while ((key = read()) != STOP_CODE) {
                  if (i2++ >= maxMapSize) throw new Error(`Property count exceeds ${maxMapSize}`);
                  object[safeKey(key)] = read();
                }
              }
              return object;
            } else {
              if (restoreMapsAsObject) {
                currentDecoder.mapsAsObjects = true;
                restoreMapsAsObject = false;
              }
              let map = /* @__PURE__ */ new Map();
              if (currentDecoder.keyMap) {
                let i2 = 0;
                while ((key = read()) != STOP_CODE) {
                  if (i2++ >= maxMapSize) {
                    throw new Error(`Map size exceeds ${maxMapSize}`);
                  }
                  map.set(currentDecoder.decodeKey(key), read());
                }
              } else {
                let i2 = 0;
                while ((key = read()) != STOP_CODE) {
                  if (i2++ >= maxMapSize) {
                    throw new Error(`Map size exceeds ${maxMapSize}`);
                  }
                  map.set(key, read());
                }
              }
              return map;
            }
          case 7:
            return STOP_CODE;
          default:
            throw new Error("Invalid major type for indefinite length " + majorType);
        }
      default:
        throw new Error("Unknown token " + token);
    }
  }
  switch (majorType) {
    case 0:
      return token;
    case 1:
      return ~token;
    case 2:
      return readBin(token);
    case 3:
      if (srcStringEnd >= position) {
        return srcString.slice(position - srcStringStart, (position += token) - srcStringStart);
      }
      if (srcStringEnd == 0 && srcEnd < 140 && token < 32) {
        let string = token < 16 ? shortStringInJS(token) : longStringInJS(token);
        if (string != null)
          return string;
      }
      return readFixedString(token);
    case 4:
      if (token >= maxArraySize) throw new Error(`Array length exceeds ${maxArraySize}`);
      let array = new Array(token);
      for (let i = 0; i < token; i++) array[i] = read();
      return array;
    case 5:
      if (token >= maxMapSize) throw new Error(`Map size exceeds ${maxArraySize}`);
      if (currentDecoder.mapsAsObjects) {
        let object = {};
        if (currentDecoder.keyMap) for (let i = 0; i < token; i++) object[safeKey(currentDecoder.decodeKey(read()))] = read();
        else for (let i = 0; i < token; i++) object[safeKey(read())] = read();
        return object;
      } else {
        if (restoreMapsAsObject) {
          currentDecoder.mapsAsObjects = true;
          restoreMapsAsObject = false;
        }
        let map = /* @__PURE__ */ new Map();
        if (currentDecoder.keyMap) for (let i = 0; i < token; i++) map.set(currentDecoder.decodeKey(read()), read());
        else for (let i = 0; i < token; i++) map.set(read(), read());
        return map;
      }
    case 6:
      if (token >= BUNDLED_STRINGS_ID) {
        let structure = currentStructures[token & 8191];
        if (structure) {
          if (!structure.read) structure.read = createStructureReader(structure);
          return structure.read();
        }
        if (token < 65536) {
          if (token == RECORD_INLINE_ID) {
            let length = readJustLength();
            let id = read();
            let structure2 = read();
            recordDefinition(id, structure2);
            let object = {};
            if (currentDecoder.keyMap) for (let i = 2; i < length; i++) {
              let key = currentDecoder.decodeKey(structure2[i - 2]);
              object[safeKey(key)] = read();
            }
            else for (let i = 2; i < length; i++) {
              let key = structure2[i - 2];
              object[safeKey(key)] = read();
            }
            return object;
          } else if (token == RECORD_DEFINITIONS_ID) {
            let length = readJustLength();
            let id = read();
            for (let i = 2; i < length; i++) {
              recordDefinition(id++, read());
            }
            return read();
          } else if (token == BUNDLED_STRINGS_ID) {
            return readBundleExt();
          }
          if (currentDecoder.getShared) {
            loadShared();
            structure = currentStructures[token & 8191];
            if (structure) {
              if (!structure.read)
                structure.read = createStructureReader(structure);
              return structure.read();
            }
          }
        }
      }
      let extension = currentExtensions[token];
      if (extension) {
        if (extension.handlesRead)
          return extension(read);
        else
          return extension(read());
      } else {
        let input = read();
        for (let i = 0; i < currentExtensionRanges.length; i++) {
          let value = currentExtensionRanges[i](token, input);
          if (value !== void 0)
            return value;
        }
        return new Tag(input, token);
      }
    case 7:
      switch (token) {
        case 20:
          return false;
        case 21:
          return true;
        case 22:
          return null;
        case 23:
          return;
        // undefined
        case 31:
        default:
          let packedValue = (packedValues || getPackedValues())[token];
          if (packedValue !== void 0)
            return packedValue;
          throw new Error("Unknown token " + token);
      }
    default:
      if (isNaN(token)) {
        let error = new Error("Unexpected end of CBOR data");
        error.incomplete = true;
        throw error;
      }
      throw new Error("Unknown CBOR token " + token);
  }
}
var validName = /^[a-zA-Z_$][a-zA-Z\d_$]*$/;
function createStructureReader(structure) {
  if (!structure) throw new Error("Structure is required in record definition");
  function readObject() {
    let length = src[position++];
    length = length & 31;
    if (length > 23) {
      switch (length) {
        case 24:
          length = src[position++];
          break;
        case 25:
          length = dataView.getUint16(position);
          position += 2;
          break;
        case 26:
          length = dataView.getUint32(position);
          position += 4;
          break;
        default:
          throw new Error("Expected array header, but got " + src[position - 1]);
      }
    }
    let compiledReader = this.compiledReader;
    while (compiledReader) {
      if (compiledReader.propertyCount === length)
        return compiledReader(read);
      compiledReader = compiledReader.next;
    }
    if (this.slowReads++ >= inlineObjectReadThreshold) {
      let array = this.length == length ? this : this.slice(0, length);
      compiledReader = currentDecoder.keyMap ? new Function("r", "return {" + array.map((k) => currentDecoder.decodeKey(k)).map((k) => validName.test(k) ? safeKey(k) + ":r()" : "[" + JSON.stringify(k) + "]:r()").join(",") + "}") : new Function("r", "return {" + array.map((key) => validName.test(key) ? safeKey(key) + ":r()" : "[" + JSON.stringify(key) + "]:r()").join(",") + "}");
      if (this.compiledReader)
        compiledReader.next = this.compiledReader;
      compiledReader.propertyCount = length;
      this.compiledReader = compiledReader;
      return compiledReader(read);
    }
    let object = {};
    if (currentDecoder.keyMap) for (let i = 0; i < length; i++) object[safeKey(currentDecoder.decodeKey(this[i]))] = read();
    else for (let i = 0; i < length; i++) {
      object[safeKey(this[i])] = read();
    }
    return object;
  }
  structure.slowReads = 0;
  return readObject;
}
function safeKey(key) {
  if (typeof key === "string") return key === "__proto__" ? "__proto_" : key;
  if (typeof key === "number" || typeof key === "boolean" || typeof key === "bigint") return key.toString();
  if (key == null) return key + "";
  throw new Error("Invalid property name type " + typeof key);
}
var readFixedString = readStringJS;
function readStringJS(length) {
  let result;
  if (length < 16) {
    if (result = shortStringInJS(length))
      return result;
  }
  if (length > 64 && decoder)
    return decoder.decode(src.subarray(position, position += length));
  const end = position + length;
  const units = [];
  result = "";
  while (position < end) {
    const byte1 = src[position++];
    if ((byte1 & 128) === 0) {
      units.push(byte1);
    } else if ((byte1 & 224) === 192) {
      const byte2 = src[position++] & 63;
      units.push((byte1 & 31) << 6 | byte2);
    } else if ((byte1 & 240) === 224) {
      const byte2 = src[position++] & 63;
      const byte3 = src[position++] & 63;
      units.push((byte1 & 31) << 12 | byte2 << 6 | byte3);
    } else if ((byte1 & 248) === 240) {
      const byte2 = src[position++] & 63;
      const byte3 = src[position++] & 63;
      const byte4 = src[position++] & 63;
      let unit = (byte1 & 7) << 18 | byte2 << 12 | byte3 << 6 | byte4;
      if (unit > 65535) {
        unit -= 65536;
        units.push(unit >>> 10 & 1023 | 55296);
        unit = 56320 | unit & 1023;
      }
      units.push(unit);
    } else {
      units.push(byte1);
    }
    if (units.length >= 4096) {
      result += fromCharCode.apply(String, units);
      units.length = 0;
    }
  }
  if (units.length > 0) {
    result += fromCharCode.apply(String, units);
  }
  return result;
}
var fromCharCode = String.fromCharCode;
function longStringInJS(length) {
  let start = position;
  let bytes = new Array(length);
  for (let i = 0; i < length; i++) {
    const byte = src[position++];
    if ((byte & 128) > 0) {
      position = start;
      return;
    }
    bytes[i] = byte;
  }
  return fromCharCode.apply(String, bytes);
}
function shortStringInJS(length) {
  if (length < 4) {
    if (length < 2) {
      if (length === 0)
        return "";
      else {
        let a = src[position++];
        if ((a & 128) > 1) {
          position -= 1;
          return;
        }
        return fromCharCode(a);
      }
    } else {
      let a = src[position++];
      let b = src[position++];
      if ((a & 128) > 0 || (b & 128) > 0) {
        position -= 2;
        return;
      }
      if (length < 3)
        return fromCharCode(a, b);
      let c = src[position++];
      if ((c & 128) > 0) {
        position -= 3;
        return;
      }
      return fromCharCode(a, b, c);
    }
  } else {
    let a = src[position++];
    let b = src[position++];
    let c = src[position++];
    let d = src[position++];
    if ((a & 128) > 0 || (b & 128) > 0 || (c & 128) > 0 || (d & 128) > 0) {
      position -= 4;
      return;
    }
    if (length < 6) {
      if (length === 4)
        return fromCharCode(a, b, c, d);
      else {
        let e = src[position++];
        if ((e & 128) > 0) {
          position -= 5;
          return;
        }
        return fromCharCode(a, b, c, d, e);
      }
    } else if (length < 8) {
      let e = src[position++];
      let f = src[position++];
      if ((e & 128) > 0 || (f & 128) > 0) {
        position -= 6;
        return;
      }
      if (length < 7)
        return fromCharCode(a, b, c, d, e, f);
      let g = src[position++];
      if ((g & 128) > 0) {
        position -= 7;
        return;
      }
      return fromCharCode(a, b, c, d, e, f, g);
    } else {
      let e = src[position++];
      let f = src[position++];
      let g = src[position++];
      let h = src[position++];
      if ((e & 128) > 0 || (f & 128) > 0 || (g & 128) > 0 || (h & 128) > 0) {
        position -= 8;
        return;
      }
      if (length < 10) {
        if (length === 8)
          return fromCharCode(a, b, c, d, e, f, g, h);
        else {
          let i = src[position++];
          if ((i & 128) > 0) {
            position -= 9;
            return;
          }
          return fromCharCode(a, b, c, d, e, f, g, h, i);
        }
      } else if (length < 12) {
        let i = src[position++];
        let j = src[position++];
        if ((i & 128) > 0 || (j & 128) > 0) {
          position -= 10;
          return;
        }
        if (length < 11)
          return fromCharCode(a, b, c, d, e, f, g, h, i, j);
        let k = src[position++];
        if ((k & 128) > 0) {
          position -= 11;
          return;
        }
        return fromCharCode(a, b, c, d, e, f, g, h, i, j, k);
      } else {
        let i = src[position++];
        let j = src[position++];
        let k = src[position++];
        let l = src[position++];
        if ((i & 128) > 0 || (j & 128) > 0 || (k & 128) > 0 || (l & 128) > 0) {
          position -= 12;
          return;
        }
        if (length < 14) {
          if (length === 12)
            return fromCharCode(a, b, c, d, e, f, g, h, i, j, k, l);
          else {
            let m = src[position++];
            if ((m & 128) > 0) {
              position -= 13;
              return;
            }
            return fromCharCode(a, b, c, d, e, f, g, h, i, j, k, l, m);
          }
        } else {
          let m = src[position++];
          let n = src[position++];
          if ((m & 128) > 0 || (n & 128) > 0) {
            position -= 14;
            return;
          }
          if (length < 15)
            return fromCharCode(a, b, c, d, e, f, g, h, i, j, k, l, m, n);
          let o = src[position++];
          if ((o & 128) > 0) {
            position -= 15;
            return;
          }
          return fromCharCode(a, b, c, d, e, f, g, h, i, j, k, l, m, n, o);
        }
      }
    }
  }
}
function readBin(length) {
  return currentDecoder.copyBuffers ? (
    // specifically use the copying slice (not the node one)
    Uint8Array.prototype.slice.call(src, position, position += length)
  ) : src.subarray(position, position += length);
}
var f32Array = new Float32Array(1);
var u8Array = new Uint8Array(f32Array.buffer, 0, 4);
function getFloat16() {
  let byte0 = src[position++];
  let byte1 = src[position++];
  let exponent = (byte0 & 127) >> 2;
  if (exponent === 31) {
    if (byte1 || byte0 & 3)
      return NaN;
    return byte0 & 128 ? -Infinity : Infinity;
  }
  if (exponent === 0) {
    let abs = ((byte0 & 3) << 8 | byte1) / (1 << 24);
    return byte0 & 128 ? -abs : abs;
  }
  u8Array[3] = byte0 & 128 | // sign bit
  (exponent >> 1) + 56;
  u8Array[2] = (byte0 & 7) << 5 | // last exponent bit and first two mantissa bits
  byte1 >> 3;
  u8Array[1] = byte1 << 5;
  u8Array[0] = 0;
  return f32Array[0];
}
var keyCache = new Array(4096);
var Tag = class {
  constructor(value, tag) {
    this.value = value;
    this.tag = tag;
  }
};
currentExtensions[0] = (dateString) => {
  return new Date(dateString);
};
currentExtensions[1] = (epochSec) => {
  return new Date(Math.round(epochSec * 1e3));
};
currentExtensions[2] = (buffer) => {
  let value = BigInt(0);
  for (let i = 0, l = buffer.byteLength; i < l; i++) {
    value = BigInt(buffer[i]) + (value << BigInt(8));
  }
  return value;
};
currentExtensions[3] = (buffer) => {
  return BigInt(-1) - currentExtensions[2](buffer);
};
currentExtensions[4] = (fraction) => {
  return +(fraction[1] + "e" + fraction[0]);
};
currentExtensions[5] = (fraction) => {
  return fraction[1] * Math.exp(fraction[0] * Math.log(2));
};
var recordDefinition = (id, structure) => {
  id = id - 57344;
  let existingStructure = currentStructures[id];
  if (existingStructure && existingStructure.isShared) {
    (currentStructures.restoreStructures || (currentStructures.restoreStructures = []))[id] = existingStructure;
  }
  currentStructures[id] = structure;
  structure.read = createStructureReader(structure);
};
currentExtensions[LEGACY_RECORD_INLINE_ID] = (data) => {
  let length = data.length;
  let structure = data[1];
  recordDefinition(data[0], structure);
  let object = {};
  for (let i = 2; i < length; i++) {
    let key = structure[i - 2];
    object[safeKey(key)] = data[i];
  }
  return object;
};
currentExtensions[14] = (value) => {
  if (bundledStrings)
    return bundledStrings[0].slice(bundledStrings.position0, bundledStrings.position0 += value);
  return new Tag(value, 14);
};
currentExtensions[15] = (value) => {
  if (bundledStrings)
    return bundledStrings[1].slice(bundledStrings.position1, bundledStrings.position1 += value);
  return new Tag(value, 15);
};
var glbl = { Error, RegExp };
currentExtensions[27] = (data) => {
  return (glbl[data[0]] || Error)(data[1], data[2]);
};
var packedTable = (read2) => {
  if (src[position++] != 132) {
    let error = new Error("Packed values structure must be followed by a 4 element array");
    if (src.length < position)
      error.incomplete = true;
    throw error;
  }
  let newPackedValues = read2();
  if (!newPackedValues || !newPackedValues.length) {
    let error = new Error("Packed values structure must be followed by a 4 element array");
    error.incomplete = true;
    throw error;
  }
  packedValues = packedValues ? newPackedValues.concat(packedValues.slice(newPackedValues.length)) : newPackedValues;
  packedValues.prefixes = read2();
  packedValues.suffixes = read2();
  return read2();
};
packedTable.handlesRead = true;
currentExtensions[51] = packedTable;
currentExtensions[PACKED_REFERENCE_TAG_ID] = (data) => {
  if (!packedValues) {
    if (currentDecoder.getShared)
      loadShared();
    else
      return new Tag(data, PACKED_REFERENCE_TAG_ID);
  }
  if (typeof data == "number")
    return packedValues[16 + (data >= 0 ? 2 * data : -2 * data - 1)];
  let error = new Error("No support for non-integer packed references yet");
  if (data === void 0)
    error.incomplete = true;
  throw error;
};
currentExtensions[28] = (read2) => {
  if (!referenceMap) {
    referenceMap = /* @__PURE__ */ new Map();
    referenceMap.id = 0;
  }
  let id = referenceMap.id++;
  let startingPosition = position;
  let token = src[position];
  let target2;
  if (token >> 5 == 4)
    target2 = [];
  else
    target2 = {};
  let refEntry = { target: target2 };
  referenceMap.set(id, refEntry);
  let targetProperties = read2();
  if (refEntry.used) {
    if (Object.getPrototypeOf(target2) !== Object.getPrototypeOf(targetProperties)) {
      position = startingPosition;
      target2 = targetProperties;
      referenceMap.set(id, { target: target2 });
      targetProperties = read2();
    }
    return Object.assign(target2, targetProperties);
  }
  refEntry.target = targetProperties;
  return targetProperties;
};
currentExtensions[28].handlesRead = true;
currentExtensions[29] = (id) => {
  let refEntry = referenceMap.get(id);
  refEntry.used = true;
  return refEntry.target;
};
currentExtensions[258] = (array) => new Set(array);
(currentExtensions[259] = (read2) => {
  if (currentDecoder.mapsAsObjects) {
    currentDecoder.mapsAsObjects = false;
    restoreMapsAsObject = true;
  }
  return read2();
}).handlesRead = true;
function combine(a, b) {
  if (typeof a === "string")
    return a + b;
  if (a instanceof Array)
    return a.concat(b);
  return Object.assign({}, a, b);
}
function getPackedValues() {
  if (!packedValues) {
    if (currentDecoder.getShared)
      loadShared();
    else
      throw new Error("No packed values available");
  }
  return packedValues;
}
var SHARED_DATA_TAG_ID = 1399353956;
currentExtensionRanges.push((tag, input) => {
  if (tag >= 225 && tag <= 255)
    return combine(getPackedValues().prefixes[tag - 224], input);
  if (tag >= 28704 && tag <= 32767)
    return combine(getPackedValues().prefixes[tag - 28672], input);
  if (tag >= 1879052288 && tag <= 2147483647)
    return combine(getPackedValues().prefixes[tag - 1879048192], input);
  if (tag >= 216 && tag <= 223)
    return combine(input, getPackedValues().suffixes[tag - 216]);
  if (tag >= 27647 && tag <= 28671)
    return combine(input, getPackedValues().suffixes[tag - 27639]);
  if (tag >= 1811940352 && tag <= 1879048191)
    return combine(input, getPackedValues().suffixes[tag - 1811939328]);
  if (tag == SHARED_DATA_TAG_ID) {
    return {
      packedValues,
      structures: currentStructures.slice(0),
      version: input
    };
  }
  if (tag == 55799)
    return input;
});
var isLittleEndianMachine = new Uint8Array(new Uint16Array([1]).buffer)[0] == 1;
var typedArrays = [
  Uint8Array,
  Uint8ClampedArray,
  Uint16Array,
  Uint32Array,
  typeof BigUint64Array == "undefined" ? { name: "BigUint64Array" } : BigUint64Array,
  Int8Array,
  Int16Array,
  Int32Array,
  typeof BigInt64Array == "undefined" ? { name: "BigInt64Array" } : BigInt64Array,
  Float32Array,
  Float64Array
];
var typedArrayTags = [64, 68, 69, 70, 71, 72, 77, 78, 79, 85, 86];
for (let i = 0; i < typedArrays.length; i++) {
  registerTypedArray(typedArrays[i], typedArrayTags[i]);
}
function registerTypedArray(TypedArray, tag) {
  let dvMethod = "get" + TypedArray.name.slice(0, -5);
  let bytesPerElement;
  if (typeof TypedArray === "function")
    bytesPerElement = TypedArray.BYTES_PER_ELEMENT;
  else
    TypedArray = null;
  for (let littleEndian = 0; littleEndian < 2; littleEndian++) {
    if (!littleEndian && bytesPerElement == 1)
      continue;
    let sizeShift = bytesPerElement == 2 ? 1 : bytesPerElement == 4 ? 2 : bytesPerElement == 8 ? 3 : 0;
    currentExtensions[littleEndian ? tag : tag - 4] = bytesPerElement == 1 || littleEndian == isLittleEndianMachine ? (buffer) => {
      if (!TypedArray)
        throw new Error("Could not find typed array for code " + tag);
      if (!currentDecoder.copyBuffers) {
        if (bytesPerElement === 1 || bytesPerElement === 2 && !(buffer.byteOffset & 1) || bytesPerElement === 4 && !(buffer.byteOffset & 3) || bytesPerElement === 8 && !(buffer.byteOffset & 7))
          return new TypedArray(buffer.buffer, buffer.byteOffset, buffer.byteLength >> sizeShift);
      }
      return new TypedArray(Uint8Array.prototype.slice.call(buffer, 0).buffer);
    } : (buffer) => {
      if (!TypedArray)
        throw new Error("Could not find typed array for code " + tag);
      let dv = new DataView(buffer.buffer, buffer.byteOffset, buffer.byteLength);
      let elements = buffer.length >> sizeShift;
      let ta = new TypedArray(elements);
      let method = dv[dvMethod];
      for (let i = 0; i < elements; i++) {
        ta[i] = method.call(dv, i << sizeShift, littleEndian);
      }
      return ta;
    };
  }
}
function readBundleExt() {
  let length = readJustLength();
  let bundlePosition = position + read();
  for (let i = 2; i < length; i++) {
    let bundleLength = readJustLength();
    position += bundleLength;
  }
  let dataPosition = position;
  position = bundlePosition;
  bundledStrings = [readStringJS(readJustLength()), readStringJS(readJustLength())];
  bundledStrings.position0 = 0;
  bundledStrings.position1 = 0;
  bundledStrings.postBundlePosition = position;
  position = dataPosition;
  return read();
}
function readJustLength() {
  let token = src[position++] & 31;
  if (token > 23) {
    switch (token) {
      case 24:
        token = src[position++];
        break;
      case 25:
        token = dataView.getUint16(position);
        position += 2;
        break;
      case 26:
        token = dataView.getUint32(position);
        position += 4;
        break;
    }
  }
  return token;
}
function loadShared() {
  if (currentDecoder.getShared) {
    let sharedData = saveState(() => {
      src = null;
      return currentDecoder.getShared();
    }) || {};
    let updatedStructures = sharedData.structures || [];
    currentDecoder.sharedVersion = sharedData.version;
    packedValues = currentDecoder.sharedValues = sharedData.packedValues;
    if (currentStructures === true)
      currentDecoder.structures = currentStructures = updatedStructures;
    else
      currentStructures.splice.apply(currentStructures, [0, updatedStructures.length].concat(updatedStructures));
  }
}
function saveState(callback) {
  let savedSrcEnd = srcEnd;
  let savedPosition = position;
  let savedStringPosition = stringPosition;
  let savedSrcStringStart = srcStringStart;
  let savedSrcStringEnd = srcStringEnd;
  let savedSrcString = srcString;
  let savedStrings = strings;
  let savedReferenceMap = referenceMap;
  let savedBundledStrings = bundledStrings;
  let savedSrc = new Uint8Array(src.slice(0, srcEnd));
  let savedStructures = currentStructures;
  let savedDecoder = currentDecoder;
  let savedSequentialMode = sequentialMode;
  let value = callback();
  srcEnd = savedSrcEnd;
  position = savedPosition;
  stringPosition = savedStringPosition;
  srcStringStart = savedSrcStringStart;
  srcStringEnd = savedSrcStringEnd;
  srcString = savedSrcString;
  strings = savedStrings;
  referenceMap = savedReferenceMap;
  bundledStrings = savedBundledStrings;
  src = savedSrc;
  sequentialMode = savedSequentialMode;
  currentStructures = savedStructures;
  currentDecoder = savedDecoder;
  dataView = new DataView(src.buffer, src.byteOffset, src.byteLength);
  return value;
}
function clearSource() {
  src = null;
  referenceMap = null;
  currentStructures = null;
}
function setSizeLimits(limits) {
  if (limits.maxMapSize) maxMapSize = limits.maxMapSize;
  if (limits.maxArraySize) maxArraySize = limits.maxArraySize;
  if (limits.maxObjectSize) maxObjectSize = limits.maxObjectSize;
}
var mult10 = new Array(147);
for (let i = 0; i < 256; i++) {
  mult10[i] = +("1e" + Math.floor(45.15 - i * 0.30103));
}
var defaultDecoder = new Decoder({ useRecords: false });
var decode = defaultDecoder.decode;
var decodeMultiple = defaultDecoder.decodeMultiple;
var FLOAT32_OPTIONS = {
  NEVER: 0,
  ALWAYS: 1,
  DECIMAL_ROUND: 3,
  DECIMAL_FIT: 4
};

// node_modules/cbor-x/encode.js
var textEncoder;
try {
  textEncoder = new TextEncoder();
} catch (error) {
}
var extensions;
var extensionClasses;
var Buffer2 = typeof globalThis === "object" && globalThis.Buffer;
var hasNodeBuffer = typeof Buffer2 !== "undefined";
var ByteArrayAllocate = hasNodeBuffer ? Buffer2.allocUnsafeSlow : Uint8Array;
var ByteArray = hasNodeBuffer ? Buffer2 : Uint8Array;
var MAX_STRUCTURES = 256;
var MAX_BUFFER_SIZE = hasNodeBuffer ? 4294967296 : 2144337920;
var throwOnIterable;
var target;
var targetView;
var position2 = 0;
var safeEnd;
var bundledStrings2 = null;
var MAX_BUNDLE_SIZE = 61440;
var hasNonLatin = /[\u0080-\uFFFF]/;
var RECORD_SYMBOL = Symbol("record-id");
var Encoder = class extends Decoder {
  constructor(options) {
    super(options);
    this.offset = 0;
    let typeBuffer;
    let start;
    let sharedStructures;
    let hasSharedUpdate;
    let structures;
    let referenceMap2;
    options = options || {};
    let encodeUtf8 = ByteArray.prototype.utf8Write ? function(string, position3, maxBytes) {
      return target.utf8Write(string, position3, maxBytes);
    } : textEncoder && textEncoder.encodeInto ? function(string, position3) {
      return textEncoder.encodeInto(string, target.subarray(position3)).written;
    } : false;
    let encoder = this;
    let hasSharedStructures = options.structures || options.saveStructures;
    let maxSharedStructures = options.maxSharedStructures;
    if (maxSharedStructures == null)
      maxSharedStructures = hasSharedStructures ? 128 : 0;
    if (maxSharedStructures > 8190)
      throw new Error("Maximum maxSharedStructure is 8190");
    let isSequential = options.sequential;
    if (isSequential) {
      maxSharedStructures = 0;
    }
    if (!this.structures)
      this.structures = [];
    if (this.saveStructures)
      this.saveShared = this.saveStructures;
    let samplingPackedValues, packedObjectMap2, sharedValues = options.sharedValues;
    let sharedPackedObjectMap2;
    if (sharedValues) {
      sharedPackedObjectMap2 = /* @__PURE__ */ Object.create(null);
      for (let i = 0, l = sharedValues.length; i < l; i++) {
        sharedPackedObjectMap2[sharedValues[i]] = i;
      }
    }
    let recordIdsToRemove = [];
    let transitionsCount = 0;
    let serializationsSinceTransitionRebuild = 0;
    this.mapEncode = function(value, encodeOptions) {
      if (this._keyMap && !this._mapped) {
        switch (value.constructor.name) {
          case "Array":
            value = value.map((r) => this.encodeKeys(r));
            break;
        }
      }
      return this.encode(value, encodeOptions);
    };
    this.encode = function(value, encodeOptions) {
      if (!target) {
        target = new ByteArrayAllocate(8192);
        targetView = new DataView(target.buffer, 0, 8192);
        position2 = 0;
      }
      safeEnd = target.length - 10;
      if (safeEnd - position2 < 2048) {
        target = new ByteArrayAllocate(target.length);
        targetView = new DataView(target.buffer, 0, target.length);
        safeEnd = target.length - 10;
        position2 = 0;
      } else if (encodeOptions === REUSE_BUFFER_MODE)
        position2 = position2 + 7 & 2147483640;
      start = position2;
      if (encoder.useSelfDescribedHeader) {
        targetView.setUint32(position2, 3654940416);
        position2 += 3;
      }
      referenceMap2 = encoder.structuredClone ? /* @__PURE__ */ new Map() : null;
      if (encoder.bundleStrings && typeof value !== "string") {
        bundledStrings2 = [];
        bundledStrings2.size = Infinity;
      } else
        bundledStrings2 = null;
      sharedStructures = encoder.structures;
      if (sharedStructures) {
        if (sharedStructures.uninitialized) {
          let sharedData = encoder.getShared() || {};
          encoder.structures = sharedStructures = sharedData.structures || [];
          encoder.sharedVersion = sharedData.version;
          let sharedValues2 = encoder.sharedValues = sharedData.packedValues;
          if (sharedValues2) {
            sharedPackedObjectMap2 = {};
            for (let i = 0, l = sharedValues2.length; i < l; i++)
              sharedPackedObjectMap2[sharedValues2[i]] = i;
          }
        }
        let sharedStructuresLength = sharedStructures.length;
        if (sharedStructuresLength > maxSharedStructures && !isSequential)
          sharedStructuresLength = maxSharedStructures;
        if (!sharedStructures.transitions) {
          sharedStructures.transitions = /* @__PURE__ */ Object.create(null);
          for (let i = 0; i < sharedStructuresLength; i++) {
            let keys = sharedStructures[i];
            if (!keys)
              continue;
            let nextTransition, transition = sharedStructures.transitions;
            for (let j = 0, l = keys.length; j < l; j++) {
              if (transition[RECORD_SYMBOL] === void 0)
                transition[RECORD_SYMBOL] = i;
              let key = keys[j];
              nextTransition = transition[key];
              if (!nextTransition) {
                nextTransition = transition[key] = /* @__PURE__ */ Object.create(null);
              }
              transition = nextTransition;
            }
            transition[RECORD_SYMBOL] = i | 1048576;
          }
        }
        if (!isSequential)
          sharedStructures.nextId = sharedStructuresLength;
      }
      if (hasSharedUpdate)
        hasSharedUpdate = false;
      structures = sharedStructures || [];
      packedObjectMap2 = sharedPackedObjectMap2;
      if (options.pack) {
        let packedValues2 = /* @__PURE__ */ new Map();
        packedValues2.values = [];
        packedValues2.encoder = encoder;
        packedValues2.maxValues = options.maxPrivatePackedValues || (sharedPackedObjectMap2 ? 16 : Infinity);
        packedValues2.objectMap = sharedPackedObjectMap2 || false;
        packedValues2.samplingPackedValues = samplingPackedValues;
        findRepetitiveStrings(value, packedValues2);
        if (packedValues2.values.length > 0) {
          target[position2++] = 216;
          target[position2++] = 51;
          writeArrayHeader(4);
          let valuesArray = packedValues2.values;
          encode2(valuesArray);
          writeArrayHeader(0);
          writeArrayHeader(0);
          packedObjectMap2 = Object.create(sharedPackedObjectMap2 || null);
          for (let i = 0, l = valuesArray.length; i < l; i++) {
            packedObjectMap2[valuesArray[i]] = i;
          }
        }
      }
      throwOnIterable = encodeOptions & THROW_ON_ITERABLE;
      try {
        if (throwOnIterable)
          return;
        encode2(value);
        if (bundledStrings2) {
          writeBundles(start, encode2);
        }
        encoder.offset = position2;
        if (referenceMap2 && referenceMap2.idsToInsert) {
          position2 += referenceMap2.idsToInsert.length * 2;
          if (position2 > safeEnd)
            makeRoom(position2);
          encoder.offset = position2;
          let serialized = insertIds(target.subarray(start, position2), referenceMap2.idsToInsert);
          referenceMap2 = null;
          return serialized;
        }
        if (encodeOptions & REUSE_BUFFER_MODE) {
          target.start = start;
          target.end = position2;
          return target;
        }
        return target.subarray(start, position2);
      } finally {
        if (sharedStructures) {
          if (serializationsSinceTransitionRebuild < 10)
            serializationsSinceTransitionRebuild++;
          if (sharedStructures.length > maxSharedStructures)
            sharedStructures.length = maxSharedStructures;
          if (transitionsCount > 1e4) {
            sharedStructures.transitions = null;
            serializationsSinceTransitionRebuild = 0;
            transitionsCount = 0;
            if (recordIdsToRemove.length > 0)
              recordIdsToRemove = [];
          } else if (recordIdsToRemove.length > 0 && !isSequential) {
            for (let i = 0, l = recordIdsToRemove.length; i < l; i++) {
              recordIdsToRemove[i][RECORD_SYMBOL] = void 0;
            }
            recordIdsToRemove = [];
          }
        }
        if (hasSharedUpdate && encoder.saveShared) {
          if (encoder.structures.length > maxSharedStructures) {
            encoder.structures = encoder.structures.slice(0, maxSharedStructures);
          }
          let returnBuffer = target.subarray(start, position2);
          if (encoder.updateSharedData() === false)
            return encoder.encode(value);
          return returnBuffer;
        }
        if (encodeOptions & RESET_BUFFER_MODE)
          position2 = start;
      }
    };
    this.findCommonStringsToPack = () => {
      samplingPackedValues = /* @__PURE__ */ new Map();
      if (!sharedPackedObjectMap2)
        sharedPackedObjectMap2 = /* @__PURE__ */ Object.create(null);
      return (options2) => {
        let threshold = options2 && options2.threshold || 4;
        let position3 = this.pack ? options2.maxPrivatePackedValues || 16 : 0;
        if (!sharedValues)
          sharedValues = this.sharedValues = [];
        for (let [key, status] of samplingPackedValues) {
          if (status.count > threshold) {
            sharedPackedObjectMap2[key] = position3++;
            sharedValues.push(key);
            hasSharedUpdate = true;
          }
        }
        while (this.saveShared && this.updateSharedData() === false) {
        }
        samplingPackedValues = null;
      };
    };
    const encode2 = (value) => {
      if (position2 > safeEnd)
        target = makeRoom(position2);
      var type = typeof value;
      var length;
      if (type === "string") {
        if (packedObjectMap2) {
          let packedPosition = packedObjectMap2[value];
          if (packedPosition >= 0) {
            if (packedPosition < 16)
              target[position2++] = packedPosition + 224;
            else {
              target[position2++] = 198;
              if (packedPosition & 1)
                encode2(15 - packedPosition >> 1);
              else
                encode2(packedPosition - 16 >> 1);
            }
            return;
          } else if (samplingPackedValues && !options.pack) {
            let status = samplingPackedValues.get(value);
            if (status)
              status.count++;
            else
              samplingPackedValues.set(value, {
                count: 1
              });
          }
        }
        let strLength = value.length;
        if (bundledStrings2 && strLength >= 4 && strLength < 1024) {
          if ((bundledStrings2.size += strLength) > MAX_BUNDLE_SIZE) {
            let extStart;
            let maxBytes2 = (bundledStrings2[0] ? bundledStrings2[0].length * 3 + bundledStrings2[1].length : 0) + 10;
            if (position2 + maxBytes2 > safeEnd)
              target = makeRoom(position2 + maxBytes2);
            target[position2++] = 217;
            target[position2++] = 223;
            target[position2++] = 249;
            target[position2++] = bundledStrings2.position ? 132 : 130;
            target[position2++] = 26;
            extStart = position2 - start;
            position2 += 4;
            if (bundledStrings2.position) {
              writeBundles(start, encode2);
            }
            bundledStrings2 = ["", ""];
            bundledStrings2.size = 0;
            bundledStrings2.position = extStart;
          }
          let twoByte = hasNonLatin.test(value);
          bundledStrings2[twoByte ? 0 : 1] += value;
          target[position2++] = twoByte ? 206 : 207;
          encode2(strLength);
          return;
        }
        let headerSize;
        if (strLength < 32) {
          headerSize = 1;
        } else if (strLength < 256) {
          headerSize = 2;
        } else if (strLength < 65536) {
          headerSize = 3;
        } else {
          headerSize = 5;
        }
        let maxBytes = strLength * 3;
        if (position2 + maxBytes > safeEnd)
          target = makeRoom(position2 + maxBytes);
        if (strLength < 64 || !encodeUtf8) {
          let i, c1, c2, strPosition = position2 + headerSize;
          for (i = 0; i < strLength; i++) {
            c1 = value.charCodeAt(i);
            if (c1 < 128) {
              target[strPosition++] = c1;
            } else if (c1 < 2048) {
              target[strPosition++] = c1 >> 6 | 192;
              target[strPosition++] = c1 & 63 | 128;
            } else if ((c1 & 64512) === 55296 && ((c2 = value.charCodeAt(i + 1)) & 64512) === 56320) {
              c1 = 65536 + ((c1 & 1023) << 10) + (c2 & 1023);
              i++;
              target[strPosition++] = c1 >> 18 | 240;
              target[strPosition++] = c1 >> 12 & 63 | 128;
              target[strPosition++] = c1 >> 6 & 63 | 128;
              target[strPosition++] = c1 & 63 | 128;
            } else {
              target[strPosition++] = c1 >> 12 | 224;
              target[strPosition++] = c1 >> 6 & 63 | 128;
              target[strPosition++] = c1 & 63 | 128;
            }
          }
          length = strPosition - position2 - headerSize;
        } else {
          length = encodeUtf8(value, position2 + headerSize, maxBytes);
        }
        if (length < 24) {
          target[position2++] = 96 | length;
        } else if (length < 256) {
          if (headerSize < 2) {
            target.copyWithin(position2 + 2, position2 + 1, position2 + 1 + length);
          }
          target[position2++] = 120;
          target[position2++] = length;
        } else if (length < 65536) {
          if (headerSize < 3) {
            target.copyWithin(position2 + 3, position2 + 2, position2 + 2 + length);
          }
          target[position2++] = 121;
          target[position2++] = length >> 8;
          target[position2++] = length & 255;
        } else {
          if (headerSize < 5) {
            target.copyWithin(position2 + 5, position2 + 3, position2 + 3 + length);
          }
          target[position2++] = 122;
          targetView.setUint32(position2, length);
          position2 += 4;
        }
        position2 += length;
      } else if (type === "number") {
        if (!this.alwaysUseFloat && value >>> 0 === value) {
          if (value < 24) {
            target[position2++] = value;
          } else if (value < 256) {
            target[position2++] = 24;
            target[position2++] = value;
          } else if (value < 65536) {
            target[position2++] = 25;
            target[position2++] = value >> 8;
            target[position2++] = value & 255;
          } else {
            target[position2++] = 26;
            targetView.setUint32(position2, value);
            position2 += 4;
          }
        } else if (!this.alwaysUseFloat && value >> 0 === value) {
          if (value >= -24) {
            target[position2++] = 31 - value;
          } else if (value >= -256) {
            target[position2++] = 56;
            target[position2++] = ~value;
          } else if (value >= -65536) {
            target[position2++] = 57;
            targetView.setUint16(position2, ~value);
            position2 += 2;
          } else {
            target[position2++] = 58;
            targetView.setUint32(position2, ~value);
            position2 += 4;
          }
        } else {
          let useFloat32;
          if ((useFloat32 = this.useFloat32) > 0 && value < 4294967296 && value >= -2147483648) {
            target[position2++] = 250;
            targetView.setFloat32(position2, value);
            let xShifted;
            if (useFloat32 < 4 || // this checks for rounding of numbers that were encoded in 32-bit float to nearest significant decimal digit that could be preserved
            (xShifted = value * mult10[(target[position2] & 127) << 1 | target[position2 + 1] >> 7]) >> 0 === xShifted) {
              position2 += 4;
              return;
            } else
              position2--;
          }
          target[position2++] = 251;
          targetView.setFloat64(position2, value);
          position2 += 8;
        }
      } else if (type === "object") {
        if (!value)
          target[position2++] = 246;
        else {
          if (referenceMap2) {
            let referee = referenceMap2.get(value);
            if (referee) {
              target[position2++] = 216;
              target[position2++] = 29;
              target[position2++] = 25;
              if (!referee.references) {
                let idsToInsert = referenceMap2.idsToInsert || (referenceMap2.idsToInsert = []);
                referee.references = [];
                idsToInsert.push(referee);
              }
              referee.references.push(position2 - start);
              position2 += 2;
              return;
            } else
              referenceMap2.set(value, { offset: position2 - start });
          }
          let constructor = value.constructor;
          if (constructor === Object) {
            writeObject(value);
          } else if (constructor === Array) {
            length = value.length;
            if (length < 24) {
              target[position2++] = 128 | length;
            } else {
              writeArrayHeader(length);
            }
            for (let i = 0; i < length; i++) {
              encode2(value[i]);
            }
          } else if (constructor === Map) {
            if (this.mapsAsObjects ? this.useTag259ForMaps !== false : this.useTag259ForMaps) {
              target[position2++] = 217;
              target[position2++] = 1;
              target[position2++] = 3;
            }
            length = value.size;
            if (length < 24) {
              target[position2++] = 160 | length;
            } else if (length < 256) {
              target[position2++] = 184;
              target[position2++] = length;
            } else if (length < 65536) {
              target[position2++] = 185;
              target[position2++] = length >> 8;
              target[position2++] = length & 255;
            } else {
              target[position2++] = 186;
              targetView.setUint32(position2, length);
              position2 += 4;
            }
            if (encoder.keyMap) {
              for (let [key, entryValue] of value) {
                encode2(encoder.encodeKey(key));
                encode2(entryValue);
              }
            } else {
              for (let [key, entryValue] of value) {
                encode2(key);
                encode2(entryValue);
              }
            }
          } else {
            for (let i = 0, l = extensions.length; i < l; i++) {
              let extensionClass = extensionClasses[i];
              if (value instanceof extensionClass) {
                let extension = extensions[i];
                let tag = extension.tag;
                if (tag == void 0)
                  tag = extension.getTag && extension.getTag.call(this, value);
                if (tag < 24) {
                  target[position2++] = 192 | tag;
                } else if (tag < 256) {
                  target[position2++] = 216;
                  target[position2++] = tag;
                } else if (tag < 65536) {
                  target[position2++] = 217;
                  target[position2++] = tag >> 8;
                  target[position2++] = tag & 255;
                } else if (tag > -1) {
                  target[position2++] = 218;
                  targetView.setUint32(position2, tag);
                  position2 += 4;
                }
                extension.encode.call(this, value, encode2, makeRoom);
                return;
              }
            }
            if (value[Symbol.iterator]) {
              if (throwOnIterable) {
                let error = new Error("Iterable should be serialized as iterator");
                error.iteratorNotHandled = true;
                throw error;
              }
              target[position2++] = 159;
              for (let entry of value) {
                encode2(entry);
              }
              target[position2++] = 255;
              return;
            }
            if (value[Symbol.asyncIterator] || isBlob(value)) {
              let error = new Error("Iterable/blob should be serialized as iterator");
              error.iteratorNotHandled = true;
              throw error;
            }
            if (this.useToJSON && value.toJSON) {
              const json = value.toJSON();
              if (json !== value)
                return encode2(json);
            }
            writeObject(value);
          }
        }
      } else if (type === "boolean") {
        target[position2++] = value ? 245 : 244;
      } else if (type === "bigint") {
        if (value < BigInt(1) << BigInt(64) && value >= 0) {
          target[position2++] = 27;
          targetView.setBigUint64(position2, value);
        } else if (value > -(BigInt(1) << BigInt(64)) && value < 0) {
          target[position2++] = 59;
          targetView.setBigUint64(position2, -value - BigInt(1));
        } else {
          if (this.largeBigIntToFloat) {
            target[position2++] = 251;
            targetView.setFloat64(position2, Number(value));
          } else {
            if (value >= BigInt(0))
              target[position2++] = 194;
            else {
              target[position2++] = 195;
              value = BigInt(-1) - value;
            }
            let bytes = [];
            while (value) {
              bytes.push(Number(value & BigInt(255)));
              value >>= BigInt(8);
            }
            writeBuffer(new Uint8Array(bytes.reverse()), makeRoom);
            return;
          }
        }
        position2 += 8;
      } else if (type === "undefined") {
        target[position2++] = 247;
      } else {
        throw new Error("Unknown type: " + type);
      }
    };
    const writeObject = this.useRecords === false ? this.variableMapSize ? (object) => {
      let keys = Object.keys(object);
      let vals = Object.values(object);
      let length = keys.length;
      if (length < 24) {
        target[position2++] = 160 | length;
      } else if (length < 256) {
        target[position2++] = 184;
        target[position2++] = length;
      } else if (length < 65536) {
        target[position2++] = 185;
        target[position2++] = length >> 8;
        target[position2++] = length & 255;
      } else {
        target[position2++] = 186;
        targetView.setUint32(position2, length);
        position2 += 4;
      }
      let key;
      if (encoder.keyMap) {
        for (let i = 0; i < length; i++) {
          encode2(encoder.encodeKey(keys[i]));
          encode2(vals[i]);
        }
      } else {
        for (let i = 0; i < length; i++) {
          encode2(keys[i]);
          encode2(vals[i]);
        }
      }
    } : (object) => {
      target[position2++] = 185;
      let objectOffset = position2 - start;
      position2 += 2;
      let size = 0;
      if (encoder.keyMap) {
        for (let key in object) if (typeof object.hasOwnProperty !== "function" || object.hasOwnProperty(key)) {
          encode2(encoder.encodeKey(key));
          encode2(object[key]);
          size++;
        }
      } else {
        for (let key in object) if (typeof object.hasOwnProperty !== "function" || object.hasOwnProperty(key)) {
          encode2(key);
          encode2(object[key]);
          size++;
        }
      }
      target[objectOffset++ + start] = size >> 8;
      target[objectOffset + start] = size & 255;
    } : (object, skipValues) => {
      let nextTransition, transition = structures.transitions || (structures.transitions = /* @__PURE__ */ Object.create(null));
      let newTransitions = 0;
      let length = 0;
      let parentRecordId;
      let keys;
      if (this.keyMap) {
        keys = Object.keys(object).map((k) => this.encodeKey(k));
        length = keys.length;
        for (let i = 0; i < length; i++) {
          let key = keys[i];
          nextTransition = transition[key];
          if (!nextTransition) {
            nextTransition = transition[key] = /* @__PURE__ */ Object.create(null);
            newTransitions++;
          }
          transition = nextTransition;
        }
      } else {
        for (let key in object) if (typeof object.hasOwnProperty !== "function" || object.hasOwnProperty(key)) {
          nextTransition = transition[key];
          if (!nextTransition) {
            if (transition[RECORD_SYMBOL] & 1048576) {
              parentRecordId = transition[RECORD_SYMBOL] & 65535;
            }
            nextTransition = transition[key] = /* @__PURE__ */ Object.create(null);
            newTransitions++;
          }
          transition = nextTransition;
          length++;
        }
      }
      let recordId = transition[RECORD_SYMBOL];
      if (recordId !== void 0) {
        recordId &= 65535;
        target[position2++] = 217;
        target[position2++] = recordId >> 8 | 224;
        target[position2++] = recordId & 255;
      } else {
        if (!keys)
          keys = transition.__keys__ || (transition.__keys__ = Object.keys(object));
        if (parentRecordId === void 0) {
          recordId = structures.nextId++;
          if (!recordId) {
            recordId = 0;
            structures.nextId = 1;
          }
          if (recordId >= MAX_STRUCTURES) {
            structures.nextId = (recordId = maxSharedStructures) + 1;
          }
        } else {
          recordId = parentRecordId;
        }
        structures[recordId] = keys;
        if (recordId < maxSharedStructures) {
          target[position2++] = 217;
          target[position2++] = recordId >> 8 | 224;
          target[position2++] = recordId & 255;
          transition = structures.transitions;
          for (let i = 0; i < length; i++) {
            if (transition[RECORD_SYMBOL] === void 0 || transition[RECORD_SYMBOL] & 1048576)
              transition[RECORD_SYMBOL] = recordId;
            transition = transition[keys[i]];
          }
          transition[RECORD_SYMBOL] = recordId | 1048576;
          hasSharedUpdate = true;
        } else {
          transition[RECORD_SYMBOL] = recordId;
          targetView.setUint32(position2, 3655335680);
          position2 += 3;
          if (newTransitions)
            transitionsCount += serializationsSinceTransitionRebuild * newTransitions;
          if (recordIdsToRemove.length >= MAX_STRUCTURES - maxSharedStructures)
            recordIdsToRemove.shift()[RECORD_SYMBOL] = void 0;
          recordIdsToRemove.push(transition);
          writeArrayHeader(length + 2);
          encode2(57344 + recordId);
          encode2(keys);
          if (skipValues) return;
          for (let key in object)
            if (typeof object.hasOwnProperty !== "function" || object.hasOwnProperty(key))
              encode2(object[key]);
          return;
        }
      }
      if (length < 24) {
        target[position2++] = 128 | length;
      } else {
        writeArrayHeader(length);
      }
      if (skipValues) return;
      for (let key in object)
        if (typeof object.hasOwnProperty !== "function" || object.hasOwnProperty(key))
          encode2(object[key]);
    };
    const makeRoom = (end) => {
      let newSize;
      if (end > 16777216) {
        if (end - start > MAX_BUFFER_SIZE)
          throw new Error("Encoded buffer would be larger than maximum buffer size");
        newSize = Math.min(
          MAX_BUFFER_SIZE,
          Math.round(Math.max((end - start) * (end > 67108864 ? 1.25 : 2), 4194304) / 4096) * 4096
        );
      } else
        newSize = (Math.max(end - start << 2, target.length - 1) >> 12) + 1 << 12;
      let newBuffer = new ByteArrayAllocate(newSize);
      targetView = new DataView(newBuffer.buffer, 0, newSize);
      if (target.copy)
        target.copy(newBuffer, 0, start, end);
      else
        newBuffer.set(target.slice(start, end));
      position2 -= start;
      start = 0;
      safeEnd = newBuffer.length - 10;
      return target = newBuffer;
    };
    let chunkThreshold = 100;
    let continuedChunkThreshold = 1e3;
    this.encodeAsIterable = function(value, options2) {
      return startEncoding(value, options2, encodeObjectAsIterable);
    };
    this.encodeAsAsyncIterable = function(value, options2) {
      return startEncoding(value, options2, encodeObjectAsAsyncIterable);
    };
    function* encodeObjectAsIterable(object, iterateProperties, finalIterable) {
      let constructor = object.constructor;
      if (constructor === Object) {
        let useRecords = encoder.useRecords !== false;
        if (useRecords)
          writeObject(object, true);
        else
          writeEntityLength(Object.keys(object).length, 160);
        for (let key in object) {
          let value = object[key];
          if (!useRecords) encode2(key);
          if (value && typeof value === "object") {
            if (iterateProperties[key])
              yield* encodeObjectAsIterable(value, iterateProperties[key]);
            else
              yield* tryEncode(value, iterateProperties, key);
          } else encode2(value);
        }
      } else if (constructor === Array) {
        let length = object.length;
        writeArrayHeader(length);
        for (let i = 0; i < length; i++) {
          let value = object[i];
          if (value && (typeof value === "object" || position2 - start > chunkThreshold)) {
            if (iterateProperties.element)
              yield* encodeObjectAsIterable(value, iterateProperties.element);
            else
              yield* tryEncode(value, iterateProperties, "element");
          } else encode2(value);
        }
      } else if (object[Symbol.iterator] && !object.buffer) {
        target[position2++] = 159;
        for (let value of object) {
          if (value && (typeof value === "object" || position2 - start > chunkThreshold)) {
            if (iterateProperties.element)
              yield* encodeObjectAsIterable(value, iterateProperties.element);
            else
              yield* tryEncode(value, iterateProperties, "element");
          } else encode2(value);
        }
        target[position2++] = 255;
      } else if (isBlob(object)) {
        writeEntityLength(object.size, 64);
        yield target.subarray(start, position2);
        yield object;
        restartEncoding();
      } else if (object[Symbol.asyncIterator]) {
        target[position2++] = 159;
        yield target.subarray(start, position2);
        yield object;
        restartEncoding();
        target[position2++] = 255;
      } else {
        encode2(object);
      }
      if (finalIterable && position2 > start) yield target.subarray(start, position2);
      else if (position2 - start > chunkThreshold) {
        yield target.subarray(start, position2);
        restartEncoding();
      }
    }
    function* tryEncode(value, iterateProperties, key) {
      let restart = position2 - start;
      try {
        encode2(value);
        if (position2 - start > chunkThreshold) {
          yield target.subarray(start, position2);
          restartEncoding();
        }
      } catch (error) {
        if (error.iteratorNotHandled) {
          iterateProperties[key] = {};
          position2 = start + restart;
          yield* encodeObjectAsIterable.call(this, value, iterateProperties[key]);
        } else throw error;
      }
    }
    function restartEncoding() {
      chunkThreshold = continuedChunkThreshold;
      encoder.encode(null, THROW_ON_ITERABLE);
    }
    function startEncoding(value, options2, encodeIterable) {
      if (options2 && options2.chunkThreshold)
        chunkThreshold = continuedChunkThreshold = options2.chunkThreshold;
      else
        chunkThreshold = 100;
      if (value && typeof value === "object") {
        encoder.encode(null, THROW_ON_ITERABLE);
        return encodeIterable(value, encoder.iterateProperties || (encoder.iterateProperties = {}), true);
      }
      return [encoder.encode(value)];
    }
    async function* encodeObjectAsAsyncIterable(value, iterateProperties) {
      for (let encodedValue of encodeObjectAsIterable(value, iterateProperties, true)) {
        let constructor = encodedValue.constructor;
        if (constructor === ByteArray || constructor === Uint8Array)
          yield encodedValue;
        else if (isBlob(encodedValue)) {
          let reader = encodedValue.stream().getReader();
          let next;
          while (!(next = await reader.read()).done) {
            yield next.value;
          }
        } else if (encodedValue[Symbol.asyncIterator]) {
          for await (let asyncValue of encodedValue) {
            restartEncoding();
            if (asyncValue)
              yield* encodeObjectAsAsyncIterable(asyncValue, iterateProperties.async || (iterateProperties.async = {}));
            else yield encoder.encode(asyncValue);
          }
        } else {
          yield encodedValue;
        }
      }
    }
  }
  useBuffer(buffer) {
    target = buffer;
    targetView = new DataView(target.buffer, target.byteOffset, target.byteLength);
    position2 = 0;
  }
  clearSharedData() {
    if (this.structures)
      this.structures = [];
    if (this.sharedValues)
      this.sharedValues = void 0;
  }
  updateSharedData() {
    let lastVersion = this.sharedVersion || 0;
    this.sharedVersion = lastVersion + 1;
    let structuresCopy = this.structures.slice(0);
    let sharedData = new SharedData(structuresCopy, this.sharedValues, this.sharedVersion);
    let saveResults = this.saveShared(
      sharedData,
      (existingShared) => (existingShared && existingShared.version || 0) == lastVersion
    );
    if (saveResults === false) {
      sharedData = this.getShared() || {};
      this.structures = sharedData.structures || [];
      this.sharedValues = sharedData.packedValues;
      this.sharedVersion = sharedData.version;
      this.structures.nextId = this.structures.length;
    } else {
      structuresCopy.forEach((structure, i) => this.structures[i] = structure);
    }
    return saveResults;
  }
};
function writeEntityLength(length, majorValue) {
  if (length < 24)
    target[position2++] = majorValue | length;
  else if (length < 256) {
    target[position2++] = majorValue | 24;
    target[position2++] = length;
  } else if (length < 65536) {
    target[position2++] = majorValue | 25;
    target[position2++] = length >> 8;
    target[position2++] = length & 255;
  } else {
    target[position2++] = majorValue | 26;
    targetView.setUint32(position2, length);
    position2 += 4;
  }
}
var SharedData = class {
  constructor(structures, values, version) {
    this.structures = structures;
    this.packedValues = values;
    this.version = version;
  }
};
function writeArrayHeader(length) {
  if (length < 24)
    target[position2++] = 128 | length;
  else if (length < 256) {
    target[position2++] = 152;
    target[position2++] = length;
  } else if (length < 65536) {
    target[position2++] = 153;
    target[position2++] = length >> 8;
    target[position2++] = length & 255;
  } else {
    target[position2++] = 154;
    targetView.setUint32(position2, length);
    position2 += 4;
  }
}
var BlobConstructor = typeof Blob === "undefined" ? function() {
} : Blob;
function isBlob(object) {
  if (object instanceof BlobConstructor)
    return true;
  let tag = object[Symbol.toStringTag];
  return tag === "Blob" || tag === "File";
}
function findRepetitiveStrings(value, packedValues2) {
  switch (typeof value) {
    case "string":
      if (value.length > 3) {
        if (packedValues2.objectMap[value] > -1 || packedValues2.values.length >= packedValues2.maxValues)
          return;
        let packedStatus = packedValues2.get(value);
        if (packedStatus) {
          if (++packedStatus.count == 2) {
            packedValues2.values.push(value);
          }
        } else {
          packedValues2.set(value, {
            count: 1
          });
          if (packedValues2.samplingPackedValues) {
            let status = packedValues2.samplingPackedValues.get(value);
            if (status)
              status.count++;
            else
              packedValues2.samplingPackedValues.set(value, {
                count: 1
              });
          }
        }
      }
      break;
    case "object":
      if (value) {
        if (value instanceof Array) {
          for (let i = 0, l = value.length; i < l; i++) {
            findRepetitiveStrings(value[i], packedValues2);
          }
        } else {
          let includeKeys = !packedValues2.encoder.useRecords;
          for (var key in value) {
            if (value.hasOwnProperty(key)) {
              if (includeKeys)
                findRepetitiveStrings(key, packedValues2);
              findRepetitiveStrings(value[key], packedValues2);
            }
          }
        }
      }
      break;
    case "function":
      console.log(value);
  }
}
var isLittleEndianMachine2 = new Uint8Array(new Uint16Array([1]).buffer)[0] == 1;
extensionClasses = [
  Date,
  Set,
  Error,
  RegExp,
  Tag,
  ArrayBuffer,
  Uint8Array,
  Uint8ClampedArray,
  Uint16Array,
  Uint32Array,
  typeof BigUint64Array == "undefined" ? function() {
  } : BigUint64Array,
  Int8Array,
  Int16Array,
  Int32Array,
  typeof BigInt64Array == "undefined" ? function() {
  } : BigInt64Array,
  Float32Array,
  Float64Array,
  SharedData
];
extensions = [
  {
    // Date
    tag: 1,
    encode(date, encode2) {
      let seconds = date.getTime() / 1e3;
      if ((this.useTimestamp32 || date.getMilliseconds() === 0) && seconds >= 0 && seconds < 4294967296) {
        target[position2++] = 26;
        targetView.setUint32(position2, seconds);
        position2 += 4;
      } else {
        target[position2++] = 251;
        targetView.setFloat64(position2, seconds);
        position2 += 8;
      }
    }
  },
  {
    // Set
    tag: 258,
    // https://github.com/input-output-hk/cbor-sets-spec/blob/master/CBOR_SETS.md
    encode(set, encode2) {
      let array = Array.from(set);
      encode2(array);
    }
  },
  {
    // Error
    tag: 27,
    // http://cbor.schmorp.de/generic-object
    encode(error, encode2) {
      encode2([error.name, error.message]);
    }
  },
  {
    // RegExp
    tag: 27,
    // http://cbor.schmorp.de/generic-object
    encode(regex, encode2) {
      encode2(["RegExp", regex.source, regex.flags]);
    }
  },
  {
    // Tag
    getTag(tag) {
      return tag.tag;
    },
    encode(tag, encode2) {
      encode2(tag.value);
    }
  },
  {
    // ArrayBuffer
    encode(arrayBuffer, encode2, makeRoom) {
      writeBuffer(arrayBuffer, makeRoom);
    }
  },
  {
    // Uint8Array
    getTag(typedArray) {
      if (typedArray.constructor === Uint8Array) {
        if (this.tagUint8Array || hasNodeBuffer && this.tagUint8Array !== false)
          return 64;
      }
    },
    encode(typedArray, encode2, makeRoom) {
      writeBuffer(typedArray, makeRoom);
    }
  },
  typedArrayEncoder(68, 1),
  typedArrayEncoder(69, 2),
  typedArrayEncoder(70, 4),
  typedArrayEncoder(71, 8),
  typedArrayEncoder(72, 1),
  typedArrayEncoder(77, 2),
  typedArrayEncoder(78, 4),
  typedArrayEncoder(79, 8),
  typedArrayEncoder(85, 4),
  typedArrayEncoder(86, 8),
  {
    encode(sharedData, encode2) {
      let packedValues2 = sharedData.packedValues || [];
      let sharedStructures = sharedData.structures || [];
      if (packedValues2.values.length > 0) {
        target[position2++] = 216;
        target[position2++] = 51;
        writeArrayHeader(4);
        let valuesArray = packedValues2.values;
        encode2(valuesArray);
        writeArrayHeader(0);
        writeArrayHeader(0);
        packedObjectMap = Object.create(sharedPackedObjectMap || null);
        for (let i = 0, l = valuesArray.length; i < l; i++) {
          packedObjectMap[valuesArray[i]] = i;
        }
      }
      if (sharedStructures) {
        targetView.setUint32(position2, 3655335424);
        position2 += 3;
        let definitions = sharedStructures.slice(0);
        definitions.unshift(57344);
        definitions.push(new Tag(sharedData.version, 1399353956));
        encode2(definitions);
      } else
        encode2(new Tag(sharedData.version, 1399353956));
    }
  }
];
function typedArrayEncoder(tag, size) {
  if (!isLittleEndianMachine2 && size > 1)
    tag -= 4;
  return {
    tag,
    encode: function writeExtBuffer(typedArray, encode2) {
      let length = typedArray.byteLength;
      let offset = typedArray.byteOffset || 0;
      let buffer = typedArray.buffer || typedArray;
      encode2(hasNodeBuffer ? Buffer2.from(buffer, offset, length) : new Uint8Array(buffer, offset, length));
    }
  };
}
function writeBuffer(buffer, makeRoom) {
  let length = buffer.byteLength;
  if (length < 24) {
    target[position2++] = 64 + length;
  } else if (length < 256) {
    target[position2++] = 88;
    target[position2++] = length;
  } else if (length < 65536) {
    target[position2++] = 89;
    target[position2++] = length >> 8;
    target[position2++] = length & 255;
  } else {
    target[position2++] = 90;
    targetView.setUint32(position2, length);
    position2 += 4;
  }
  if (position2 + length >= target.length) {
    makeRoom(position2 + length);
  }
  target.set(buffer.buffer ? buffer : new Uint8Array(buffer), position2);
  position2 += length;
}
function insertIds(serialized, idsToInsert) {
  let nextId;
  let distanceToMove = idsToInsert.length * 2;
  let lastEnd = serialized.length - distanceToMove;
  idsToInsert.sort((a, b) => a.offset > b.offset ? 1 : -1);
  for (let id = 0; id < idsToInsert.length; id++) {
    let referee = idsToInsert[id];
    referee.id = id;
    for (let position3 of referee.references) {
      serialized[position3++] = id >> 8;
      serialized[position3] = id & 255;
    }
  }
  while (nextId = idsToInsert.pop()) {
    let offset = nextId.offset;
    serialized.copyWithin(offset + distanceToMove, offset, lastEnd);
    distanceToMove -= 2;
    let position3 = offset + distanceToMove;
    serialized[position3++] = 216;
    serialized[position3++] = 28;
    lastEnd = offset;
  }
  return serialized;
}
function writeBundles(start, encode2) {
  targetView.setUint32(bundledStrings2.position + start, position2 - bundledStrings2.position - start + 1);
  let writeStrings = bundledStrings2;
  bundledStrings2 = null;
  encode2(writeStrings[0]);
  encode2(writeStrings[1]);
}
var defaultEncoder = new Encoder({ useRecords: false });
var encode = defaultEncoder.encode;
var encodeAsIterable = defaultEncoder.encodeAsIterable;
var encodeAsAsyncIterable = defaultEncoder.encodeAsAsyncIterable;
var { NEVER, ALWAYS, DECIMAL_ROUND, DECIMAL_FIT } = FLOAT32_OPTIONS;
var REUSE_BUFFER_MODE = 512;
var RESET_BUFFER_MODE = 1024;
var THROW_ON_ITERABLE = 2048;

// cordis-sdk/src/types.ts
var CordisError = class extends Error {
  constructor(code, message, detail = null) {
    super(message);
    this.code = code;
    this.detail = detail;
    this.name = "CordisError";
  }
  wire() {
    return { code: this.code, message: this.message, detail: this.detail };
  }
};
function fields(value) {
  if (!(value instanceof Map)) throw new CordisError(2, "\u534F\u8BAE\u8BB0\u5F55\u9700\u8981 CBOR \u6620\u5C04");
  return Object.fromEntries(value);
}
function identifier(value) {
  if ((typeof value !== "number" || !Number.isSafeInteger(value)) && typeof value !== "bigint") {
    throw new CordisError(2, "\u7F16\u53F7\u9700\u8981\u65E0\u7B26\u53F7\u6574\u6570");
  }
  const result = BigInt(value);
  if (result <= 0n || result > 0xffffffffffffffffn) throw new CordisError(2, "\u7F16\u53F7\u8D85\u51FA u64 \u8303\u56F4");
  return result;
}
function configuration(value) {
  if (value instanceof Map)
    return Object.fromEntries(Array.from(value, ([key, item]) => [key, configuration(item)]));
  if (Array.isArray(value)) return value.map(configuration);
  return value;
}

// cordis-sdk/src/budget.ts
function checkValue(root, limits) {
  const pending = [[root, 0]];
  let nodes = 0;
  while (pending.length) {
    const [value, depth] = pending.pop();
    if (++nodes > limits.maxValues || depth > limits.maxDepth)
      throw new CordisError(8, "\u503C\u56FE\u8D85\u8FC7\u6DF1\u5EA6\u6216\u8282\u70B9\u9884\u7B97");
    if (value instanceof Tag) pending.push([value.value, depth + 1]);
    else if (Array.isArray(value)) {
      if (value.length > limits.maxCollection) throw new CordisError(8, "\u6570\u7EC4\u8D85\u8FC7\u5143\u7D20\u9884\u7B97");
      for (const item of value) pending.push([item, depth + 1]);
    } else if (value instanceof Map) {
      if (value.size > limits.maxCollection) throw new CordisError(8, "\u6620\u5C04\u8D85\u8FC7\u5143\u7D20\u9884\u7B97");
      for (const [key, item] of value) pending.push([key, depth + 1], [item, depth + 1]);
    } else if (value && typeof value === "object" && !(value instanceof Uint8Array)) {
      const entries = Object.entries(value);
      if (entries.length > limits.maxCollection) throw new CordisError(8, "\u8BB0\u5F55\u8D85\u8FC7\u5B57\u6BB5\u9884\u7B97");
      for (const [key, item] of entries) pending.push([key, depth + 1], [item, depth + 1]);
    }
  }
}

// cordis-sdk/src/wire.ts
var DEFAULT_LIMITS = {
  maxFrame: 16777216,
  maxInflight: 64,
  queueCapacity: 64,
  timeoutMs: 3e4,
  consecutiveTimeouts: 3,
  maxDepth: 64,
  maxCollection: 65536,
  maxValues: 262144
};
var Connection = class {
  constructor(runtime, input, output, limits) {
    this.runtime = runtime;
    this.input = input;
    this.output = output;
    this.limits = { ...DEFAULT_LIMITS, ...limits };
    setSizeLimits({
      maxArraySize: this.limits.maxCollection + 1,
      maxMapSize: this.limits.maxCollection + 1,
      maxObjectSize: this.limits.maxCollection + 1
    });
  }
  limits;
  pending = /* @__PURE__ */ new Map();
  running = /* @__PURE__ */ new Map();
  decoder = new Decoder({ mapsAsObjects: false });
  encoder = new Encoder({
    useRecords: false,
    mapsAsObjects: false,
    tagUint8Array: false
  });
  closed = false;
  next = 1n;
  timeouts = 0;
  writes = 0;
  tail = Promise.resolve();
  /** DOC-16 §6：队列容量和编码失败归还本轮出口，写入失败终止连接。 */
  async send(message, offers = []) {
    let committed = false;
    try {
      if (this.closed) throw new CordisError(4, "\u6838\u5FC3\u8FDE\u63A5\u5DF2\u5173\u95ED");
      if (this.writes >= this.limits.queueCapacity)
        throw new CordisError(8, "\u53D1\u9001\u961F\u5217\u989D\u5EA6\u8017\u5C3D");
      checkValue(message, this.limits);
      const data = this.encoder.encode(message);
      if (data.length > this.limits.maxFrame) throw new CordisError(8, "\u6D88\u606F\u8D85\u8FC7\u5E27\u957F\u5EA6\u4E0A\u9650");
      const framed = new Uint8Array(data.length + 4);
      new DataView(framed.buffer).setUint32(0, data.length);
      framed.set(data, 4);
      this.writes++;
      const write = this.tail.then(async () => {
        if (this.closed) throw new CordisError(4, "\u6838\u5FC3\u8FDE\u63A5\u5DF2\u5173\u95ED");
        await new Promise((resolve, reject) => {
          this.output.write(framed, (error) => error ? reject(error) : resolve());
        });
        committed = true;
      }).finally(() => {
        this.writes--;
      });
      this.tail = write;
      await write;
    } finally {
      if (!committed) this.runtime.releaseExports(offers);
    }
  }
  /** DOC-16 §10：超时释放等待表并交回 cancel；迟到载荷进入释放路径。 */
  async request(method, params, offers = []) {
    const signal = currentSignal();
    if (signal?.aborted) {
      this.runtime.releaseExports(offers);
      throw new CordisError(7, "\u8C03\u7528\u5DF2\u53D6\u6D88");
    }
    if (this.pending.size >= this.limits.maxInflight) {
      this.runtime.releaseExports(offers);
      throw new CordisError(8, "\u5728\u9014\u8BF7\u6C42\u989D\u5EA6\u8017\u5C3D");
    }
    const id = this.next++;
    let delivered = false;
    let cancelled = false;
    let rejectResponse;
    let timer;
    let cancel;
    const result = new Promise((resolve, reject) => {
      rejectResponse = reject;
      this.pending.set(id, { resolve, reject });
      cancel = () => {
        cancelled = true;
        reject(new CordisError(7, "\u8C03\u7528\u5DF2\u53D6\u6D88"));
        if (delivered) this.notifySoon("cancel", { id });
      };
      signal?.addEventListener("abort", cancel, { once: true });
      timer = setTimeout(() => {
        cancelled = true;
        reject(new CordisError(6, "\u8BF7\u6C42\u8D85\u8FC7\u622A\u6B62\u65F6\u95F4"));
        if (delivered) this.notifySoon("cancel", { id });
        if (++this.timeouts >= this.limits.consecutiveTimeouts)
          this.fail(new CordisError(6, "\u8FDE\u7EED\u8BF7\u6C42\u8D85\u65F6"));
      }, this.limits.timeoutMs);
    });
    const completed = result.finally(() => {
      clearTimeout(timer);
      this.pending.delete(id);
      signal?.removeEventListener("abort", cancel);
    });
    try {
      const sent = this.send({ request: { id, method, params } }, offers).then(() => {
        delivered = true;
        if (cancelled) this.notifySoon("cancel", { id });
      });
      const [, payload] = await Promise.all([sent, completed]);
      this.timeouts = 0;
      return payload;
    } catch (error) {
      rejectResponse(error);
      throw error;
    }
  }
  notify(method, params) {
    return this.send({ notification: { method, params } });
  }
  /** DOC-16 §6：析构控制消息发送失败使连接退役并释放整表。 */
  notifySoon(method, params) {
    if (!this.closed) void this.notify(method, params).catch((error) => this.fail(error));
  }
  fail(error, report = true) {
    if (this.closed) return;
    this.closed = true;
    for (const pending of this.pending.values()) pending.reject(error);
    this.pending.clear();
    for (const running of this.running.values()) running.abort.abort(error);
    this.runtime.close();
    this.input.destroy(
      report ? error instanceof Error ? error : new Error(String(error)) : void 0
    );
  }
  /** CP-10：流结束释放资源；帧长度检查发生在等待完整消息之前。 */
  async serve() {
    let buffered = new Uint8Array(0);
    try {
      for await (const chunk of this.input) {
        const joined = new Uint8Array(buffered.length + chunk.length);
        joined.set(buffered);
        joined.set(chunk, buffered.length);
        buffered = joined;
        while (buffered.length >= 4) {
          const length = new DataView(buffered.buffer, buffered.byteOffset, 4).getUint32(
            0
          );
          if (length > this.limits.maxFrame)
            throw new CordisError(8, "\u8F93\u5165\u5E27\u8D85\u8FC7\u957F\u5EA6\u4E0A\u9650");
          if (buffered.length < length + 4) break;
          const message = this.decoder.decode(buffered.subarray(4, length + 4));
          checkValue(message, this.limits);
          buffered = buffered.subarray(length + 4);
          this.receive(message);
        }
      }
      if (buffered.length) throw new CordisError(2, "\u8F93\u5165\u5E27\u88AB\u622A\u65AD");
    } finally {
      this.fail(new CordisError(4, "\u6838\u5FC3\u8F93\u5165\u5DF2\u7ED3\u675F"), false);
    }
  }
  receive(message) {
    if (!(message instanceof Map) || message.size !== 1)
      throw new CordisError(2, "\u6D88\u606F\u5305\u7EDC\u9700\u8981\u5355\u952E\u6620\u5C04");
    const [kind, entry] = [...message][0];
    const body = fields(entry);
    if (kind === "response") this.response(body);
    else if (kind === "request") this.start(body);
    else if (kind === "notification") {
      void this.runtime.notification(body.method, body.params).catch((error) => this.fail(error));
    } else throw new CordisError(3, "\u6D88\u606F\u7C7B\u522B\u65E0\u6548");
  }
  response(body) {
    const id = identifier(body.id);
    const result = fields(body.result);
    const pending = this.pending.get(id);
    if ("error" in result) {
      const error = fields(result.error);
      pending?.reject(new CordisError(error.code, error.message, error.detail));
    } else if ("ok" in result) {
      const payload = this.runtime.payload(result.ok);
      if (pending) pending.resolve(payload);
      else payload.close();
    } else throw new CordisError(2, "\u54CD\u5E94\u9700\u8981 ok \u6216 error");
  }
  start(body) {
    const id = identifier(body.id);
    if (this.running.has(id)) throw new CordisError(2, "\u8BF7\u6C42\u7F16\u53F7\u91CD\u590D");
    if (this.running.size >= this.limits.maxInflight)
      throw new CordisError(8, "\u5165\u7AD9\u8BF7\u6C42\u989D\u5EA6\u8017\u5C3D");
    const abort = new AbortController();
    this.running.set(id, { method: body.method, abort });
    void this.execute(id, body, abort.signal).catch((error) => this.fail(error));
  }
  async execute(id, body, signal) {
    const payload = this.runtime.payload(body.params);
    let offers = [];
    try {
      let result;
      try {
        const response = await this.runtime.request(body.method, payload, signal);
        offers = response.offers;
        result = { ok: response.value };
      } catch (problem) {
        const error = problem instanceof CordisError ? problem : new CordisError(problem instanceof TypeError ? 2 : 1, String(problem));
        result = { error: error.wire() };
      }
      try {
        await this.send({ response: { id, result } }, offers);
      } catch (problem) {
        if (!(problem instanceof CordisError) || problem.code !== 8) throw problem;
        await this.send({ response: { id, result: { error: problem.wire() } } });
      }
    } finally {
      payload.close();
      this.running.delete(id);
    }
  }
};

// cordis-sdk/src/resources.ts
var OwnedResource = class {
  exports = 0;
  retain() {
    this.exports++;
  }
  release() {
    if (--this.exports === 0) this.close();
  }
};

// cordis-sdk/src/bindings.ts
var services = /* @__PURE__ */ new WeakMap();
var references = /* @__PURE__ */ new WeakMap();
var finalizers = new FinalizationRegistry((reference) => reference.release());
var Reference = class {
  constructor(runtime, number) {
    this.runtime = runtime;
    this.number = number;
  }
  holders = 0;
  retain() {
    this.holders++;
    return this;
  }
  release() {
    if (--this.holders === 0) {
      this.runtime.imports.delete(this.number);
      this.runtime.connection.notifySoon("release", { objects: [this.number] });
    }
  }
};
var Payload = class {
  constructor(runtime, value) {
    this.runtime = runtime;
    this.value = value;
    if (!(value instanceof Map)) return;
    for (const entry of value.get("refs") ?? []) {
      const number = identifier(entry);
      if (this.references.has(number)) throw new CordisError(2, "refs \u4E2D\u7684\u7F16\u53F7\u91CD\u590D");
      this.references.set(number, runtime.reference(number).retain());
    }
  }
  references = /* @__PURE__ */ new Map();
  reference(number) {
    const result = this.references.get(identifier(number));
    if (!result) throw new CordisError(4, "\u8F7D\u8377\u7F3A\u5C11\u8D44\u6E90\u6301\u6709\u6743");
    return result;
  }
  close() {
    for (const reference of this.references.values()) reference.release();
    this.references.clear();
  }
};
var Service = class _Service {
  constructor(provider, binding, interfaceName, scoped = false) {
    this.provider = provider;
    this.binding = binding;
    this.interfaceName = interfaceName;
    this.scoped = scoped;
    const descriptor = binding.SCHEMA.services.find(
      (service) => service.interface === interfaceName
    );
    if (!descriptor) throw new CordisError(3, `\u63A5\u53E3\u63CF\u8FF0\u7F3A\u5E2D\uFF1A${interfaceName}`);
    this.methods = descriptor.methods.map((method) => method.name);
    if (scoped) this.methods.push("cordis.scope");
  }
  methods;
  exported() {
    const provider = this.provider instanceof Remote ? this.provider.clone() : this.provider;
    return new _Service(provider, this.binding, this.interfaceName, this.scoped);
  }
  close() {
    if (this.provider instanceof Remote) this.provider.close();
    if (this.provider instanceof OwnedResource) this.provider.release();
  }
  retain() {
    if (this.provider instanceof OwnedResource) this.provider.retain();
  }
  async invoke(runtime, method, args, payload) {
    const transport = new BindingTransport(runtime, this.binding);
    if (method === "cordis.scope" && this.scoped) {
      if (args.length !== 1) throw new CordisError(2, "cordis.scope \u9700\u8981\u4E00\u9879\u5143\u6570\u636E");
      const view = await this.provider.scope(args[0]);
      return transport.exportResource(view, this.interfaceName, false);
    }
    if (!this.methods.includes(method)) throw new CordisError(3, "\u63A5\u53E3\u65B9\u6CD5\u7F3A\u5E2D");
    const values = transport.incoming(args, payload);
    let provider = this.provider;
    if (provider instanceof Remote) {
      const descriptor = this.binding.SCHEMA.services.find(
        (service) => service.interface === this.interfaceName
      );
      const client = this.binding[`${descriptor.class}Client`];
      provider = new client(provider);
    }
    return this.binding.dispatch(provider, transport, this.interfaceName, method, values);
  }
};
var BindingTransport = class {
  constructor(runtime, binding) {
    this.runtime = runtime;
    this.binding = binding;
  }
  invoke(_service, _method, _args, _asynchronous) {
    return Promise.reject(new CordisError(4, "\u8C03\u7528\u9700\u8981\u5DF2\u53D6\u5F97\u7684\u8FDC\u7A0B\u670D\u52A1"));
  }
  exportResource(value, service, _borrowed = false) {
    const provider = value?.transport ?? value;
    const tag = new this.binding.Tagged(6e4, 0);
    services.set(tag, new Service(provider, this.binding, service));
    return tag;
  }
  importResource(value, service, _borrowed = false) {
    const reference = references.get(value);
    if (!reference) throw new CordisError(4, "\u8D44\u6E90\u6807\u7B7E\u7F3A\u5C11\u62E5\u6709\u5173\u7CFB");
    const remote = new Remote(this.runtime, this.binding, reference, service);
    if (finalizers.unregister(value)) reference.release();
    return remote;
  }
  /** ADR-0039：标签保留自身份额，生成 Client 解码后取得独立持有。 */
  incoming(value, payload) {
    if (value instanceof Tag) {
      const tag = new this.binding.Tagged(value.tag, this.incoming(value.value, payload));
      if (value.tag === 6e4) {
        const reference = payload.reference(value.value).retain();
        references.set(tag, reference);
        finalizers.register(tag, reference, tag);
      }
      return tag;
    }
    if (value instanceof Map)
      return new Map(
        Array.from(value, ([key, item]) => [
          this.incoming(key, payload),
          this.incoming(item, payload)
        ])
      );
    if (Array.isArray(value)) return value.map((item) => this.incoming(item, payload));
    return value;
  }
};
var Remote = class _Remote extends BindingTransport {
  constructor(runtime, binding, reference, interfaceName) {
    super(runtime, binding);
    this.reference = reference;
    this.interfaceName = interfaceName;
    reference.retain();
    finalizers.register(this, reference, this);
  }
  active = true;
  clone() {
    if (!this.active) throw new CordisError(4, "\u670D\u52A1\u4EE3\u7406\u5DF2\u91CA\u653E");
    return new _Remote(this.runtime, this.binding, this.reference, this.interfaceName);
  }
  async invoke(_service, method, args, _asynchronous = true) {
    if (!this.active) throw new CordisError(4, "\u670D\u52A1\u4EE3\u7406\u5DF2\u91CA\u653E");
    const offers = [];
    const values = outgoing(this.runtime, args, offers);
    const payload = await this.runtime.connection.request(
      "invoke",
      {
        object: this.reference.number,
        method,
        args: values,
        refs: offers
      },
      offers
    );
    try {
      return this.incoming(fields(payload.value).value, payload);
    } finally {
      payload.close();
    }
  }
  close() {
    if (!this.active) return;
    this.active = false;
    finalizers.unregister(this);
    this.reference.release();
  }
  [Symbol.dispose]() {
    this.close();
  }
};
function outgoing(runtime, value, offers) {
  try {
    return transform(runtime, value, offers);
  } catch (error) {
    runtime.releaseExports(offers);
    throw error;
  }
}
function transform(runtime, value, offers) {
  if (value && typeof value === "object" && "tag" in value && "value" in value) {
    const service = services.get(value);
    if (service) {
      const number = runtime.offer(service.exported());
      offers.push(number);
      return new Tag(number, 6e4);
    }
    if (value.tag === 6e4) throw new CordisError(2, "\u8D44\u6E90\u6807\u7B7E\u9700\u8981 exportResource");
    return new Tag(transform(runtime, value.value, offers), value.tag);
  }
  if (value instanceof Map)
    return new Map(
      Array.from(value, ([key, item]) => [
        transform(runtime, key, offers),
        transform(runtime, item, offers)
      ])
    );
  if (Array.isArray(value)) return value.map((item) => transform(runtime, item, offers));
  if (value && typeof value === "object" && !(value instanceof Uint8Array)) {
    return new Map(
      Object.entries(value).map(([key, item]) => [key, transform(runtime, item, offers)])
    );
  }
  return value;
}
function close(client) {
  const transport = client instanceof Remote ? client : client.transport;
  if (!(transport instanceof Remote)) throw new CordisError(2, "close \u9700\u8981\u8FDC\u7A0B Client");
  transport.close();
}

// cordis-sdk/src/context.ts
function defineComponent(component) {
  return component;
}
var Frozen = class {
  frozen = false;
  check() {
    if (this.frozen) throw new CordisError(4, "\u72B6\u6001\u5BFC\u51FA\u671F\u95F4\u5199\u5165\u51BB\u7ED3");
  }
};
var Callback = class {
  constructor(function_, context, event = false, binding, cleanup = false) {
    this.function_ = function_;
    this.context = context;
    this.event = event;
    this.binding = binding;
    this.cleanup = cleanup;
  }
};
var Exporter = class {
  constructor(function_, frozen, thaw) {
    this.function_ = function_;
    this.frozen = frozen;
    this.thaw = thaw;
  }
};
var effects = new FinalizationRegistry(
  ({ runtime, number }) => {
    runtime.connection.notifySoon("effect_release", { effect: number });
  }
);
var Effect = class {
  constructor(runtime, number) {
    this.runtime = runtime;
    this.number = number;
    effects.register(this, { runtime, number }, this);
  }
  active = true;
  async settled() {
    const payload = await this.runtime.connection.request("effect_settled", {
      effect: this.number
    });
    payload.close();
  }
  async dispose() {
    const payload = await this.runtime.connection.request("effect_dispose", {
      effect: this.number
    });
    payload.close();
  }
  close() {
    if (!this.active) return;
    this.active = false;
    effects.unregister(this);
    this.runtime.connection.notifySoon("effect_release", { effect: this.number });
  }
  [Symbol.dispose]() {
    this.close();
  }
};
var views = new FinalizationRegistry((reference) => reference.release());
var Context = class _Context {
  constructor(runtime, number, config, inherited, scope = 0n, reference, signal) {
    this.runtime = runtime;
    this.number = number;
    this.config = config;
    this.inherited = inherited;
    this.scope = scope;
    this.reference = reference;
    this.signal = signal;
    if (reference) {
      reference.retain();
      views.register(this, reference, this);
    }
  }
  async request(method, params = {}) {
    const payload = await this.runtime.connection.request(method, {
      context: this.number,
      ...params
    });
    try {
      return payload.value;
    } finally {
      payload.close();
    }
  }
  async register(method, resource, key, params) {
    const number = this.runtime.offer(resource);
    const payload = await this.runtime.connection.request(
      method,
      {
        context: this.number,
        [key]: number,
        refs: [number],
        ...params
      },
      [number]
    );
    try {
      return payload.value;
    } finally {
      payload.close();
    }
  }
  async provide(interfaceName, provider, binding, scoped = false) {
    const service = new Service(provider, binding, interfaceName, scoped);
    await this.register("provide", service, "object", {
      interface: interfaceName,
      methods: service.methods,
      scoped
    });
  }
  async get(interfaceName, binding, client) {
    const payload = await this.runtime.connection.request("get", {
      context: this.number,
      interface: interfaceName
    });
    try {
      return new client(
        new Remote(
          this.runtime,
          binding,
          payload.reference(fields(payload.value).object),
          interfaceName
        )
      );
    } finally {
      payload.close();
    }
  }
  service(interfaceName, provider, binding) {
    return new BindingTransport(this.runtime, binding).exportResource(
      provider,
      interfaceName,
      false
    );
  }
  async effect(body, label = "typescript-effect", asynchronous = false) {
    const result = fields(
      await this.register("effect", new Callback(body, this), "body", {
        label,
        asynchronous
      })
    );
    return new Effect(this.runtime, identifier(result.effect));
  }
  async defer(cleanup, asynchronous = true) {
    if (!this.scope) throw new CordisError(2, "defer \u9700\u8981\u6548\u5E94\u56DE\u8C03\u4F5C\u7528\u57DF");
    await this.register(
      "defer",
      new Callback(cleanup, this, false, void 0, true),
      "callback",
      { scope: this.scope, asynchronous }
    );
  }
  async on(event, callback, asynchronous = false, binding) {
    await this.register("on", new Callback(callback, this, true, binding), "callback", {
      event,
      asynchronous
    });
  }
  async emit(event, value = null, asynchronous = false) {
    const offers = [];
    const payload = await this.runtime.connection.request(
      "emit",
      {
        context: this.number,
        event,
        payload: outgoing(this.runtime, value, offers),
        asynchronous,
        refs: offers
      },
      offers
    );
    payload.close();
  }
  async exportState(schema, version, exporter, frozen = new Frozen(), thaw) {
    await this.register("export_state", new Exporter(exporter, frozen, thaw), "exporter", {
      schema,
      version
    });
    return frozen;
  }
  async isCurrent() {
    return this.request("is_current");
  }
  async inheritedState() {
    return this.request("inherited_state");
  }
  async configuration() {
    return configuration(fields(await this.request("config")).value);
  }
  async log(message, level = 2) {
    await this.request("log", { message, level });
  }
  async derive(isolate, intercept) {
    const payload = await this.runtime.connection.request("context_derive", {
      context: this.number,
      isolate,
      intercept
    });
    try {
      const value = fields(payload.value);
      return new _Context(
        this.runtime,
        identifier(value.context),
        this.config,
        this.inherited,
        0n,
        payload.reference(value.object)
      );
    } finally {
      payload.close();
    }
  }
  isolate(interfaceName, realm = null) {
    return this.derive([{ interface: interfaceName, realm }], []);
  }
  intercept(interfaceName, meta) {
    return this.derive([], [{ interface: interfaceName, meta }]);
  }
  async plugin(component, config = null) {
    return identifier(fields(await this.request("plugin", { component, config })).instance);
  }
  /** DOC-16 §8：任务接受取消信号，效应清理等待后台执行结束。 */
  async task(function_, label = "typescript-task") {
    return this.effect(async (scope) => {
      const abort = new AbortController();
      const pending = function_(abort.signal);
      await scope.defer(async () => {
        abort.abort();
        try {
          await pending;
        } catch (error) {
          if (!(error instanceof Error) || error.name !== "AbortError") throw error;
        }
      });
    }, label);
  }
  close() {
    if (this.reference && views.unregister(this)) this.reference.release();
  }
  [Symbol.dispose]() {
    this.close();
  }
};

// cordis-sdk/src/runtime.ts
var invocation = {
  current: () => void 0,
  run: (_signal, operation) => operation()
};
function currentSignal() {
  return invocation.current();
}
var Runtime = class {
  constructor(name, version, components, input, output, limits = {}) {
    this.name = name;
    this.version = version;
    this.components = new Map(components.map((component) => [component.name, component]));
    this.connection = new Connection(this, input, output, limits);
  }
  connection;
  components;
  exports = /* @__PURE__ */ new Map();
  imports = /* @__PURE__ */ new Map();
  contexts = /* @__PURE__ */ new Map();
  events = /* @__PURE__ */ new Map();
  next = 1n;
  description() {
    const components = [...this.components.values()].map((component) => ({
      name: component.name,
      inject: Object.entries(component.inject ?? {}).map(([interfaceName, required]) => ({
        interface: interfaceName,
        required
      })),
      provides: component.provides ?? [],
      listens: component.listens ?? [],
      state_schema: component.stateSchema ?? "",
      state_version_min: component.stateVersionMin ?? 0,
      state_version_max: component.stateVersionMax ?? 0
    }));
    return {
      protocol: 1,
      plugins: [
        {
          name: this.name,
          version: this.version,
          sdk_version: "0.1.0",
          build_info: "TypeScript UP/1 SDK",
          format: 2,
          library: null,
          components
        }
      ]
    };
  }
  payload(value) {
    return new Payload(this, value);
  }
  reference(number) {
    let reference = this.imports.get(number);
    if (!reference) {
      reference = new Reference(this, number);
      this.imports.set(number, reference);
    }
    return reference;
  }
  offer(resource) {
    if (resource instanceof Service) resource.retain();
    const id = this.next++;
    this.exports.set(id, resource);
    return id;
  }
  releaseExports(numbers) {
    for (const number of numbers) {
      const object = this.exports.get(number);
      this.exports.delete(number);
      if (object instanceof Service) object.close();
    }
  }
  object(number) {
    const object = this.exports.get(identifier(number));
    if (!object) throw new CordisError(4, "\u5BF9\u8C61\u5DF2\u9000\u5F79");
    return object;
  }
  async request(method, payload, signal) {
    if (method === "shutdown") return { value: null, offers: [] };
    const params = fields(payload.value);
    let value = null;
    if (method === "hello") {
      if (params.protocol !== 1) throw new CordisError(3, "\u9700\u8981 UP/1 \u534F\u8BAE");
      value = this.description();
    } else if (method === "resolve" || method === "apply") await this.component(method, params);
    else if (method === "invoke") {
      const service = this.object(params.object);
      if (!(service instanceof Service)) throw new CordisError(2, "invoke \u9700\u8981\u670D\u52A1\u5BF9\u8C61");
      const result = await invocation.run(
        signal,
        () => service.invoke(this, params.method, params.args, payload)
      );
      if (signal.aborted) throw new CordisError(7, "\u8C03\u7528\u5DF2\u53D6\u6D88");
      const offers = [];
      return { value: { value: outgoing(this, result, offers), refs: offers }, offers };
    } else if (method === "object_info") {
      const service = this.object(params.object);
      if (!(service instanceof Service)) throw new CordisError(2, "object_info \u9700\u8981\u670D\u52A1\u5BF9\u8C61");
      value = { interface: service.interfaceName, methods: service.methods, local: null };
    } else if (method === "callback") await this.callback(params, payload, signal);
    else if (method === "export" || method === "thaw")
      value = await this.state(method, params.object);
    else throw new CordisError(3, `\u8BF7\u6C42\u65B9\u6CD5\u7F3A\u5E2D\uFF1A${method}`);
    return { value, offers: [] };
  }
  resolveComponent(params) {
    if (params.plugin !== this.name) throw new CordisError(3, "\u63D2\u4EF6\u540D\u79F0\u4E0E\u8FDB\u7A0B\u63CF\u8FF0\u4E0D\u7B26");
    const component = this.components.get(params.component);
    if (!component) throw new CordisError(3, "\u7EC4\u4EF6\u540D\u79F0\u7F3A\u5E2D");
    return component;
  }
  async component(method, params) {
    const component = this.resolveComponent(params);
    const config = configuration(params.config);
    if (method === "resolve") {
      await component.resolve?.(config);
      return;
    }
    const context = new Context(this, identifier(params.context), config, params.inherited);
    this.contexts.set(context.number, context);
    await component.apply(context, config);
  }
  async state(method, object) {
    const exporter = this.object(object);
    if (!(exporter instanceof Exporter)) throw new CordisError(2, "\u72B6\u6001\u64CD\u4F5C\u9700\u8981\u5BFC\u51FA\u5668");
    if (method === "thaw") {
      exporter.frozen.frozen = false;
      await exporter.thaw?.();
      return null;
    }
    exporter.frozen.frozen = true;
    try {
      return { value: await exporter.function_() };
    } catch (error) {
      exporter.frozen.frozen = false;
      throw error;
    }
  }
  async callback(params, payload, signal) {
    const callback = this.object(params.object);
    if (!(callback instanceof Callback)) throw new CordisError(2, "callback \u9700\u8981\u56DE\u8C03\u5BF9\u8C61");
    const base = callback.context;
    const context = new Context(
      this,
      identifier(params.context),
      base.config,
      base.inherited,
      BigInt(params.scope),
      base.reference,
      signal
    );
    const value = callback.binding ? new BindingTransport(this, callback.binding).incoming(params.payload, payload) : params.payload;
    try {
      await callback.function_(context, callback.event ? value : void 0);
    } finally {
      context.close();
    }
  }
  async notification(method, value) {
    const params = fields(value);
    if (method === "release") this.releaseExports(params.objects.map(identifier));
    else if (method === "cancel") {
      const running = this.connection.running.get(identifier(params.id));
      if (running?.method === "invoke") running.abort.abort(new CordisError(7, "\u8C03\u7528\u5DF2\u53D6\u6D88"));
    } else if (method === "release_context") {
      const id = identifier(params.context);
      this.contexts.get(id)?.close();
      this.contexts.delete(id);
      for (const abort of this.events.get(id) ?? []) abort.abort();
      this.events.delete(id);
    } else if (method === "callback") this.event(params, value);
    else throw new CordisError(3, `\u901A\u77E5\u65B9\u6CD5\u7F3A\u5E2D\uFF1A${method}`);
  }
  event(params, value) {
    const payload = this.payload(value);
    const id = identifier(params.context);
    const abort = new AbortController();
    let events = this.events.get(id);
    if (!events) {
      events = /* @__PURE__ */ new Set();
      this.events.set(id, events);
    }
    events.add(abort);
    void this.callback(params, payload, abort.signal).catch((error) => {
      this.connection.notifySoon("log", {
        context: id,
        level: 0,
        message: String(error)
      });
    }).finally(() => {
      events.delete(abort);
      payload.close();
    });
  }
  close() {
    for (const events of this.events.values()) for (const abort of events) abort.abort();
    for (const context of this.contexts.values()) context.close();
    this.releaseExports([...this.exports.keys()]);
    this.events.clear();
    this.contexts.clear();
    this.imports.clear();
  }
};

// cordis-web/src/runtime.ts
var WebRuntime = class extends Runtime {
  modules = /* @__PURE__ */ new Map();
  active = /* @__PURE__ */ new Map();
  nextModule = 1n;
  retiredModules = 0;
  completedCleanups = /* @__PURE__ */ new WeakSet();
  pendingCleanups = /* @__PURE__ */ new Map();
  cleanupComplete = Promise.resolve();
  constructor(socket, limits = {}) {
    super("cordis-web", "0.1.0", [], socket, socket, limits);
  }
  async load(entry) {
    const url = new URL(entry.url, location.href);
    if (url.origin !== location.origin) throw new CordisError(9, "\u7EC4\u4EF6\u5165\u53E3\u9700\u8981\u540C\u6E90\u5730\u5740");
    const namespace = await import(url.href);
    const component = namespace.default;
    if (!component || typeof component.apply !== "function") throw new CordisError(2, "\u7F51\u9875\u6A21\u5757\u9700\u8981\u5BFC\u51FA\u7EC4\u4EF6\u5B9A\u4E49");
    const module = { id: this.nextModule++, entry, component: { ...component, name: entry.component } };
    this.modules.set(module.id, module);
    return module;
  }
  async initialize(entries) {
    for (const entry of entries) {
      const module = await this.load(entry);
      this.active.set(entry.name, module.id);
    }
  }
  descriptor(module) {
    const component = module.component;
    return {
      name: module.entry.name,
      version: module.entry.version,
      sdk_version: "0.1.0",
      build_info: new URL(module.entry.url, location.href).pathname,
      format: 2,
      library: module.id,
      components: [{
        name: component.name,
        inject: Object.entries(component.inject ?? {}).map(([interfaceName, required]) => ({ interface: interfaceName, required })),
        provides: component.provides ?? [],
        listens: component.listens ?? [],
        state_schema: component.stateSchema ?? "",
        state_version_min: component.stateVersionMin ?? 0,
        state_version_max: component.stateVersionMax ?? 0
      }]
    };
  }
  description() {
    return { protocol: 1, plugins: [...this.active.values()].map((id) => this.descriptor(this.modules.get(id))) };
  }
  resolveComponent(params) {
    const module = this.modules.get(identifier(params.library));
    if (!module || module.entry.name !== params.plugin || module.component.name !== params.component) throw new CordisError(4, "\u7F51\u9875\u6A21\u5757\u7248\u672C\u5DF2\u9000\u5F79");
    return module.component;
  }
  async request(method, payload, signal) {
    if (method === "callback") {
      const callback = this.object(fields(payload.value).object);
      if (callback instanceof Callback && callback.cleanup) {
        if (this.completedCleanups.has(callback)) return { value: null, offers: [] };
        const previous = this.pendingCleanups.get(callback);
        if (previous) return previous;
        const pending = super.request(method, payload, signal).then((result) => {
          this.completedCleanups.add(callback);
          return result;
        });
        this.pendingCleanups.set(callback, pending);
        try {
          return await pending;
        } finally {
          this.pendingCleanups.delete(callback);
        }
      }
    }
    if (!method.startsWith("library_")) return super.request(method, payload, signal);
    const params = fields(payload.value);
    if (method === "library_prepare") {
      const url = new URL(params.path, location.href);
      const entry = {
        name: url.searchParams.get("cordis_plugin") ?? "",
        version: url.searchParams.get("cordis_version") ?? "",
        component: url.searchParams.get("cordis_component") ?? "",
        mount: url.searchParams.get("cordis_mount") ?? "/",
        url: url.href
      };
      const module2 = await this.load(entry);
      return { value: { library: module2.id, plugin: this.descriptor(module2) }, offers: [] };
    }
    const id = identifier(params.library);
    const module = this.modules.get(id);
    if (!module) throw new CordisError(4, "\u7F51\u9875\u6A21\u5757\u7F16\u53F7\u5DF2\u9000\u5F79");
    if (method === "library_activate") {
      this.active.set(module.entry.name, id);
      return { value: null, offers: [] };
    }
    if (method === "library_retire") {
      this.modules.delete(id);
      if (this.active.get(module.entry.name) === id) this.active.delete(module.entry.name);
      return { value: { leases: 0, closed: false, quiescence_ms: 0, retired_images: ++this.retiredModules }, offers: [] };
    }
    throw new CordisError(3, "\u7F51\u9875\u6A21\u5757\u64CD\u4F5C\u7F3A\u5E2D");
  }
  /** DOC-15 AU-15：连接退役执行本宿主尚持有的本地清理，完成后进入下一条连接。 */
  close() {
    const callbacks = [...this.exports.values()].filter((value) => value instanceof Callback && value.cleanup).reverse();
    super.close();
    this.cleanupComplete = (async () => {
      for (const callback of callbacks) {
        try {
          const pending = this.pendingCleanups.get(callback);
          if (pending) await pending;
          if (!this.completedCleanups.has(callback)) {
            await callback.function_(callback.context);
            this.completedCleanups.add(callback);
          }
        } catch (error) {
          console.error("\u9875\u9762\u8D44\u6E90\u6E05\u7406\u5931\u8D25", error);
        }
      }
    })();
  }
};

// cordis-web/src/socket.ts
var Socket = class {
  constructor(socket, maxFrame, capacity) {
    this.socket = socket;
    this.maxFrame = maxFrame;
    this.capacity = capacity;
    socket.binaryType = "arraybuffer";
    socket.addEventListener("message", (event) => this.receive(event.data));
    socket.addEventListener("close", () => this.destroy());
    socket.addEventListener("error", () => this.destroy(new CordisError(4, "\u7F51\u9875\u8FDE\u63A5\u4E2D\u65AD")));
  }
  frames = [];
  waiter;
  ended = false;
  failure;
  receive(data) {
    if (!(data instanceof ArrayBuffer)) {
      this.destroy(new CordisError(2, "WebSocket \u9700\u8981\u4E8C\u8FDB\u5236\u6D88\u606F"));
      return;
    }
    if (data.byteLength > this.maxFrame || this.frames.length >= this.capacity) {
      this.destroy(new CordisError(8, "\u7F51\u9875\u8F93\u5165\u8D85\u8FC7\u6D88\u606F\u6216\u961F\u5217\u9884\u7B97"));
      return;
    }
    const frame = new Uint8Array(data.byteLength + 4);
    new DataView(frame.buffer).setUint32(0, data.byteLength);
    frame.set(new Uint8Array(data), 4);
    if (this.waiter) {
      const waiter = this.waiter;
      this.waiter = void 0;
      waiter.resolve({ value: frame, done: false });
    } else this.frames.push(frame);
  }
  write(frame, callback) {
    if (this.ended || this.socket.readyState !== WebSocket.OPEN) {
      callback(new CordisError(4, "\u7F51\u9875\u8FDE\u63A5\u5DF2\u5173\u95ED"));
      return;
    }
    if (this.socket.bufferedAmount + frame.length > this.maxFrame * this.capacity) {
      callback(new CordisError(8, "\u7F51\u9875\u53D1\u9001\u961F\u5217\u989D\u5EA6\u8017\u5C3D"));
      return;
    }
    try {
      this.socket.send(frame.subarray(4));
      callback();
    } catch (error) {
      callback(error instanceof Error ? error : new Error(String(error)));
    }
  }
  [Symbol.asyncIterator]() {
    return { next: () => {
      const frame = this.frames.shift();
      if (frame) return Promise.resolve({ value: frame, done: false });
      if (this.failure) return Promise.reject(this.failure);
      if (this.ended) return Promise.resolve({ value: void 0, done: true });
      return new Promise((resolve, reject) => {
        this.waiter = { resolve, reject };
      });
    } };
  }
  destroy(error) {
    if (this.ended) return;
    this.ended = true;
    this.failure = error;
    this.frames.length = 0;
    if (this.waiter) {
      if (error) this.waiter.reject(error);
      else this.waiter.resolve({ value: void 0, done: true });
      this.waiter = void 0;
    }
    if (this.socket.readyState < WebSocket.CLOSING) this.socket.close(error ? 1002 : 1e3);
  }
};

// cordis-sdk/src/metadata.ts
function caller(metadata) {
  const value = metadata.get("cordis.caller");
  if (value === void 0) return null;
  if (!(value instanceof Map) || value.size !== 1) throw new CordisError(2, "\u8C03\u7528\u8EAB\u4EFD\u9700\u8981 instance \u6216 page_of");
  if (typeof value.get("instance") === "string") return { instance: value.get("instance") };
  if (typeof value.get("page_of") === "string") return { page_of: value.get("page_of") };
  throw new CordisError(2, "\u8C03\u7528\u8EAB\u4EFD\u540D\u79F0\u9700\u8981\u5B57\u7B26\u4E32");
}

// cordis-web/src/index.ts
function connect(options) {
  let stopped = false;
  let current;
  let wake;
  const close2 = () => {
    stopped = true;
    current?.destroy();
    wake?.();
  };
  const onPageHide = () => close2();
  window.addEventListener("pagehide", onPageHide, { once: true });
  const completed = (async () => {
    const attempts = options.reconnectAttempts ?? 20;
    let token = options.token;
    for (let attempt = 0; !stopped && attempt <= attempts; attempt++) {
      options.status?.("connecting");
      let runtime;
      try {
        if (attempt > 0) {
          const session = await fetch("/_cordis/session.json", { cache: "no-store" });
          if (!session.ok) throw new CordisError(session.status, "\u8BFB\u53D6\u5F53\u524D\u4F1A\u8BDD\u5931\u8D25");
          token = (await session.json()).token;
        }
        const response = await fetch(options.manifest ?? "/_cordis/manifest.json", { headers: { Authorization: `Bearer ${token}` }, cache: "no-store" });
        if (!response.ok) throw new CordisError(response.status, "\u8BFB\u53D6\u7F51\u9875\u63D2\u4EF6\u6E05\u5355\u5931\u8D25");
        const entries = await response.json();
        const url = new URL(options.endpoint ?? "/_cordis/socket", location.href);
        url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
        const websocket = new WebSocket(url, ["cordis-up1", `cordis-token.${token}`]);
        current = new Socket(websocket, DEFAULT_LIMITS.maxFrame, DEFAULT_LIMITS.queueCapacity);
        runtime = new WebRuntime(current);
        await runtime.initialize(entries);
        await new Promise((resolve, reject) => {
          if (websocket.readyState === WebSocket.OPEN) {
            resolve();
            return;
          }
          websocket.addEventListener("open", () => resolve(), { once: true });
          websocket.addEventListener("error", () => reject(new CordisError(4, "WebSocket \u8FDE\u63A5\u5931\u8D25")), { once: true });
        });
        options.status?.("active");
        await runtime.connection.serve();
      } catch (error) {
        current?.destroy();
        options.status?.("disconnected", error);
      } finally {
        await runtime?.cleanupComplete;
      }
      if (!stopped && attempt < attempts) await new Promise((resolve) => {
        const timer = setTimeout(resolve, options.reconnectDelayMs ?? 500);
        wake = () => {
          clearTimeout(timer);
          resolve();
        };
      });
    }
    window.removeEventListener("pagehide", onPageHide);
    options.status?.("closed");
  })();
  return { close: close2, completed };
}
export {
  BindingTransport,
  Context,
  CordisError,
  Effect,
  Frozen,
  Remote,
  WebRuntime,
  caller,
  close,
  connect,
  defineComponent
};
