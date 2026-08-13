import Foundation

/// A JSON document, compared and printed the same way every time.
///
/// The conformance corpus stores expectations as JSON and every driver produces JSON to
/// compare against them, so equality across languages is equality of a canonical
/// serialization rather than of two structures nobody can line up. It is also the shape
/// the scenario corpora will take once the vocabulary has a protobuf schema, whose
/// canonical JSON mapping this deliberately resembles.
public enum JSONValue: Equatable, Sendable {
  case null
  case bool(Bool)
  case number(Double)
  case string(String)
  case array([JSONValue])
  case object([String: JSONValue])

  /// Reads whatever `JSONSerialization` produced.
  public init(_ value: Any) {
    switch value {
    case is NSNull: self = .null
    case let bool as Bool where Self.isBoolean(value): self = .bool(bool)
    case let number as NSNumber: self = .number(number.doubleValue)
    case let string as String: self = .string(string)
    case let array as [Any]: self = .array(array.map(JSONValue.init))
    case let object as [String: Any]: self = .object(object.mapValues(JSONValue.init))
    default: self = .null
    }
  }

  // JSON's true and false arrive as NSNumber, indistinguishable from 1 and 0 by casting
  // alone. Without this check `{"focused": true}` reads as the number 1 and prints as one.
  private static func isBoolean(_ value: Any) -> Bool {
    CFGetTypeID(value as CFTypeRef) == CFBooleanGetTypeID()
  }

  public subscript(key: String) -> JSONValue? {
    guard case .object(let fields) = self else { return nil }
    return fields[key]
  }

  public var stringValue: String? {
    guard case .string(let value) = self else { return nil }
    return value
  }

  public var boolValue: Bool? {
    guard case .bool(let value) = self else { return nil }
    return value
  }

  public var intValue: Int? {
    guard case .number(let value) = self else { return nil }
    return Int(value)
  }

  public var arrayValue: [JSONValue]? {
    guard case .array(let value) = self else { return nil }
    return value
  }

  /// The strings in an array field, or an empty array when the field is absent.
  ///
  /// Absent and empty mean the same thing for the lists in this corpus - no modifiers
  /// held, no bytes produced - so requiring `"modifiers": []` on most cases would be
  /// noise in the file for no gain in precision.
  public func strings(_ key: String) -> [String] {
    self[key]?.arrayValue?.compactMap(\.stringValue) ?? []
  }

  /// One line, keys sorted, so two renderings of equal values are equal text.
  public var rendered: String {
    switch self {
    case .null: "null"
    case .bool(let value): value ? "true" : "false"
    case .number(let value):
      value == value.rounded() && abs(value) < 1e15
        ? String(Int(value)) : String(value)
    case .string(let value): "\"\(value)\""
    case .array(let values): "[" + values.map(\.rendered).joined(separator: ", ") + "]"
    case .object(let fields):
      "{" + fields.keys.sorted().map { "\($0): \(fields[$0]!.rendered)" }.joined(separator: ", ")
        + "}"
    }
  }
}

extension JSONValue {
  /// Builds an object, dropping nils so a driver can describe a variant without spelling
  /// out the fields that do not apply to it.
  public static func fields(_ pairs: [String: JSONValue?]) -> JSONValue {
    .object(pairs.compactMapValues { $0 })
  }
}

extension JSONValue: ExpressibleByStringLiteral {
  public init(stringLiteral value: String) { self = .string(value) }
}
