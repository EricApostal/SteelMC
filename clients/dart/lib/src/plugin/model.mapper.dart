// coverage:ignore-file
// GENERATED CODE - DO NOT MODIFY BY HAND
// dart format off
// ignore_for_file: type=lint
// ignore_for_file: invalid_use_of_protected_member
// ignore_for_file: unused_element, unnecessary_cast, override_on_non_overriding_member
// ignore_for_file: strict_raw_type, inference_failure_on_untyped_parameter

part of 'model.dart';

class Vec3Mapper extends ClassMapperBase<Vec3> {
  Vec3Mapper._();

  static Vec3Mapper? _instance;
  static Vec3Mapper ensureInitialized() {
    if (_instance == null) {
      MapperContainer.globals.use(_instance = Vec3Mapper._());
    }
    return _instance!;
  }

  @override
  final String id = 'Vec3';

  static double _$x(Vec3 v) => v.x;
  static const Field<Vec3, double> _f$x = Field('x', _$x);
  static double _$y(Vec3 v) => v.y;
  static const Field<Vec3, double> _f$y = Field('y', _$y);
  static double _$z(Vec3 v) => v.z;
  static const Field<Vec3, double> _f$z = Field('z', _$z);

  @override
  final MappableFields<Vec3> fields = const {#x: _f$x, #y: _f$y, #z: _f$z};

  static Vec3 _instantiate(DecodingData data) {
    return Vec3(data.dec(_f$x), data.dec(_f$y), data.dec(_f$z));
  }

  @override
  final Function instantiate = _instantiate;

  static Vec3 fromMap(Map<String, dynamic> map) {
    return ensureInitialized().decodeMap<Vec3>(map);
  }

  static Vec3 fromJson(String json) {
    return ensureInitialized().decodeJson<Vec3>(json);
  }
}

mixin Vec3Mappable {
  String toJson() {
    return Vec3Mapper.ensureInitialized().encodeJson<Vec3>(this as Vec3);
  }

  Map<String, dynamic> toMap() {
    return Vec3Mapper.ensureInitialized().encodeMap<Vec3>(this as Vec3);
  }

  Vec3CopyWith<Vec3, Vec3, Vec3> get copyWith =>
      _Vec3CopyWithImpl<Vec3, Vec3>(this as Vec3, $identity, $identity);
  @override
  String toString() {
    return Vec3Mapper.ensureInitialized().stringifyValue(this as Vec3);
  }

  @override
  bool operator ==(Object other) {
    return Vec3Mapper.ensureInitialized().equalsValue(this as Vec3, other);
  }

  @override
  int get hashCode {
    return Vec3Mapper.ensureInitialized().hashValue(this as Vec3);
  }
}

extension Vec3ValueCopy<$R, $Out> on ObjectCopyWith<$R, Vec3, $Out> {
  Vec3CopyWith<$R, Vec3, $Out> get $asVec3 =>
      $base.as((v, t, t2) => _Vec3CopyWithImpl<$R, $Out>(v, t, t2));
}

abstract class Vec3CopyWith<$R, $In extends Vec3, $Out>
    implements ClassCopyWith<$R, $In, $Out> {
  $R call({double? x, double? y, double? z});
  Vec3CopyWith<$R2, $In, $Out2> $chain<$R2, $Out2>(Then<$Out2, $R2> t);
}

class _Vec3CopyWithImpl<$R, $Out> extends ClassCopyWithBase<$R, Vec3, $Out>
    implements Vec3CopyWith<$R, Vec3, $Out> {
  _Vec3CopyWithImpl(super.value, super.then, super.then2);

  @override
  late final ClassMapperBase<Vec3> $mapper = Vec3Mapper.ensureInitialized();
  @override
  $R call({double? x, double? y, double? z}) => $apply(
    FieldCopyWithData({
      if (x != null) #x: x,
      if (y != null) #y: y,
      if (z != null) #z: z,
    }),
  );
  @override
  Vec3 $make(CopyWithData data) => Vec3(
    data.get(#x, or: $value.x),
    data.get(#y, or: $value.y),
    data.get(#z, or: $value.z),
  );

  @override
  Vec3CopyWith<$R2, Vec3, $Out2> $chain<$R2, $Out2>(Then<$Out2, $R2> t) =>
      _Vec3CopyWithImpl<$R2, $Out2>($value, $cast, t);
}

