import CoreBluetooth

/// 스펙 4.1 의 값과 반드시 일치해야 한다. Rust `ble/peripheral.rs` 의 상수와 같은 값이다.
public enum MirrorUUIDs {
    public static let service  = CBUUID(string: "07A98A35-16C7-4BBA-A296-E28B78B7E683")
    public static let info     = CBUUID(string: "F494FC3B-ED50-4561-AADE-1A310C5732E6")
    public static let auth     = CBUUID(string: "1403603A-4C78-4899-A2B8-FDA198101900")
    public static let snapshot = CBUUID(string: "0AE789AA-EF38-4A35-9E72-A7CD7AD995D5")
    public static let triggers = CBUUID(string: "4F60A8C2-F181-4717-AEE3-07C4D7846597")
}
