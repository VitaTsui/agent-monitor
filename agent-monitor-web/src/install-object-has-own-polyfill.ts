const O = Object as ObjectConstructor & {
  hasOwn?: (object: object, property: PropertyKey) => boolean;
};

if (typeof O.hasOwn !== "function") {
  Object.defineProperty(Object, "hasOwn", {
    value(object: object, property: PropertyKey) {
      if (object == null) {
        throw new TypeError("Cannot convert undefined or null to object");
      }
      return Object.prototype.hasOwnProperty.call(Object(object), property);
    },
    configurable: true,
    enumerable: false,
    writable: true,
  });
}
