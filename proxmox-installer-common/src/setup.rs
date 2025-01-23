use std::{
    cmp,
    collections::{BTreeMap, HashMap},
    fmt,
    fs::File,
    io::{self, BufReader},
    net::IpAddr,
    path::{Path, PathBuf},
    process::{self, Command, Stdio},
};

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use crate::{
    options::{
        BtrfsBootdiskOptions, BtrfsCompressOption, Disk, FsType, ZfsBootdiskOptions,
        ZfsChecksumOption, ZfsCompressOption,
    },
    utils::CidrAddress,
};

#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxmoxProduct {
    PVE,
    PBS,
    PMG,
}

impl ProxmoxProduct {
    pub fn default_hostname(self) -> &'static str {
        match self {
            Self::PVE => "pve",
            Self::PMG => "pmg",
            Self::PBS => "pbs",
        }
    }
}

impl fmt::Display for ProxmoxProduct {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::PVE => "pve",
            Self::PMG => "pmg",
            Self::PBS => "pbs",
        })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProductConfig {
    pub fullname: String,
    pub product: ProxmoxProduct,
    #[serde(deserialize_with = "deserialize_bool_from_int")]
    pub enable_btrfs: bool,
}

impl ProductConfig {
    /// A mocked ProductConfig simulating a Proxmox VE environment.
    pub fn mocked() -> Self {
        Self {
            fullname: String::from("Proxmox VE (mocked)"),
            product: ProxmoxProduct::PVE,
            enable_btrfs: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IsoInfo {
    pub release: String,
    pub isorelease: String,
}

impl IsoInfo {
    /// A mocked IsoInfo with some edge case to convey that this is not necessarily purely numeric.
    pub fn mocked() -> Self {
        Self {
            release: String::from("42.1"),
            isorelease: String::from("mocked-1"),
        }
    }
}

/// Paths in the ISO environment containing installer data.
#[derive(Clone, Deserialize)]
pub struct IsoLocations {
    pub iso: PathBuf,
    pub lib: PathBuf,
}

impl IsoLocations {
    /// A mocked location, uses the current working directory by default
    pub fn mocked() -> Self {
        let lib = match std::env::current_dir() {
            Ok(mut dir) => {
                dir.push("run");
                dir
            }
            Err(_) => "/dev/null".into(),
        };

        Self {
            iso: std::env::current_dir().unwrap_or("/dev/null".into()),
            lib,
        }
    }
}

#[derive(Clone, Deserialize)]
pub struct SetupInfo {
    #[serde(rename = "product-cfg")]
    pub config: ProductConfig,
    #[serde(rename = "iso-info")]
    pub iso_info: IsoInfo,
    pub locations: IsoLocations,
}

impl SetupInfo {
    /// Return a mocked SetupInfo that is very similar to how our actual ones look like and should
    /// be good enough for testing.
    pub fn mocked() -> Self {
        Self {
            config: ProductConfig::mocked(),
            iso_info: IsoInfo::mocked(),
            locations: IsoLocations::mocked(),
        }
    }
}

#[derive(Clone, Deserialize)]
pub struct CountryInfo {
    pub name: String,
    #[serde(default)]
    pub zone: String,
    pub kmap: String,
}

#[derive(Clone, Deserialize, Eq, PartialEq)]
pub struct KeyboardMapping {
    pub name: String,
    #[serde(rename = "kvm")]
    pub id: String,
    #[serde(rename = "x11")]
    pub xkb_layout: String,
    #[serde(rename = "x11var")]
    pub xkb_variant: String,
}

impl cmp::PartialOrd for KeyboardMapping {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl cmp::Ord for KeyboardMapping {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.name.cmp(&other.name)
    }
}

#[derive(Clone, Deserialize)]
pub struct LocaleInfo {
    #[serde(deserialize_with = "deserialize_cczones_map")]
    pub cczones: HashMap<String, Vec<String>>,
    #[serde(rename = "country")]
    pub countries: HashMap<String, CountryInfo>,
    pub kmap: HashMap<String, KeyboardMapping>,
}

/// Fetches basic information needed for the installer which is required to work
pub fn installer_setup(in_test_mode: bool) -> Result<(SetupInfo, LocaleInfo, RuntimeInfo), String> {
    let base_path = if in_test_mode {
        format!("./testdir/{}", crate::RUNTIME_DIR)
    } else {
        crate::RUNTIME_DIR.to_owned()
    };

    load_installer_setup_files(base_path)
}

pub fn load_installer_setup_files(
    runtime_dir: impl AsRef<Path>,
) -> Result<(SetupInfo, LocaleInfo, RuntimeInfo), String> {
    let installer_info: SetupInfo = {
        let mut path = runtime_dir.as_ref().to_path_buf();
        path.push("iso-info.json");

        read_json(&path)
            .map_err(|err| format!("Failed to retrieve setup info: {}: {err}", path.display()))?
    };

    let locale_info = {
        let mut path = installer_info.locations.lib.clone();
        path.push("locale-info.json");

        read_json(&path)
            .map_err(|err| format!("Failed to retrieve locale info: {}: {err}", path.display()))?
    };

    let mut runtime_info: RuntimeInfo = {
        let mut path = runtime_dir.as_ref().to_path_buf();
        path.push("run-env-info.json");

        read_json(&path).map_err(|err| {
            format!(
                "Failed to retrieve runtime environment info: {}: {err}",
                path.display()
            )
        })?
    };

    runtime_info.disks.sort();
    if runtime_info.disks.is_empty() {
        Err("The installer could not find any supported hard disks.".to_owned())
    } else if runtime_info.network.interfaces.is_empty() {
        Err("The installer could not find any supported network interface cards.".to_owned())
    } else {
        Ok((installer_info, locale_info, runtime_info))
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct InstallZfsOption {
    pub ashift: usize,
    pub compress: ZfsCompressOption,
    pub checksum: ZfsChecksumOption,
    pub copies: usize,
    pub arc_max: usize,
}

impl From<ZfsBootdiskOptions> for InstallZfsOption {
    fn from(opts: ZfsBootdiskOptions) -> Self {
        InstallZfsOption {
            ashift: opts.ashift,
            compress: opts.compress,
            checksum: opts.checksum,
            copies: opts.copies,
            arc_max: opts.arc_max,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct InstallBtrfsOption {
    #[serde(serialize_with = "serialize_as_display")]
    pub compress: BtrfsCompressOption,
}

impl From<BtrfsBootdiskOptions> for InstallBtrfsOption {
    fn from(opts: BtrfsBootdiskOptions) -> Self {
        InstallBtrfsOption {
            compress: opts.compress,
        }
    }
}

pub fn read_json<T: for<'de> Deserialize<'de>, P: AsRef<Path>>(path: P) -> Result<T, String> {
    let file = File::open(path).map_err(|err| err.to_string())?;
    let reader = BufReader::new(file);

    serde_json::from_reader(reader).map_err(|err| format!("failed to parse JSON: {err}"))
}

fn deserialize_bool_from_int<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let val: u32 = Deserialize::deserialize(deserializer)?;
    Ok(val != 0)
}

fn deserialize_bool_from_int_maybe<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    let val: Option<u32> = Deserialize::deserialize(deserializer)?;
    Ok(val.map(|v| v != 0))
}

fn deserialize_cczones_map<'de, D>(
    deserializer: D,
) -> Result<HashMap<String, Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let map: HashMap<String, HashMap<String, u32>> = Deserialize::deserialize(deserializer)?;

    let mut result = HashMap::new();
    for (cc, list) in map.into_iter() {
        result.insert(cc, list.into_keys().collect());
    }

    Ok(result)
}

fn deserialize_disks_map<'de, D>(deserializer: D) -> Result<Vec<Disk>, D::Error>
where
    D: Deserializer<'de>,
{
    let disks =
        <Vec<(usize, String, f64, String, Option<usize>, String)>>::deserialize(deserializer)?;
    Ok(disks
        .into_iter()
        .map(
            |(index, device, size_mb, model, logical_bsize, _syspath)| Disk {
                index: index.to_string(),
                // Linux always reports the size of block devices in sectors, where one sector is
                // defined as being 2^9 = 512 bytes in size.
                // https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/linux/blk_types.h?h=v6.4#n30
                size: (size_mb * 512.) / 1024. / 1024. / 1024.,
                block_size: logical_bsize,
                path: device,
                model: (!model.is_empty()).then_some(model),
            },
        )
        .collect())
}

fn deserialize_cidr_list<'de, D>(deserializer: D) -> Result<Option<Vec<CidrAddress>>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct CidrDescriptor {
        address: String,
        prefix: usize,
        // family is implied anyway by parsing the address
    }

    let list: Vec<CidrDescriptor> = Deserialize::deserialize(deserializer)?;

    let mut result = Vec::with_capacity(list.len());
    for desc in list {
        let ip_addr = desc
            .address
            .parse::<IpAddr>()
            .map_err(|err| de::Error::custom(format!("{:?}", err)))?;

        result.push(
            CidrAddress::new(ip_addr, desc.prefix)
                .map_err(|err| de::Error::custom(format!("{:?}", err)))?,
        );
    }

    Ok(Some(result))
}

fn serialize_as_display<S, T>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: fmt::Display,
{
    serializer.collect_str(value)
}

fn serialize_bool_as_u32<S>(value: &bool, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_u32(if *value { 1 } else { 0 })
}

#[derive(Clone, Deserialize)]
pub struct RuntimeInfo {
    /// Whether is system was booted in (legacy) BIOS or UEFI mode.
    pub boot_type: BootType,

    /// Detected country if available.
    pub country: Option<String>,

    /// Maps devices to their information.
    #[serde(deserialize_with = "deserialize_disks_map")]
    pub disks: Vec<Disk>,

    /// Network addresses, gateways and DNS info.
    pub network: NetworkInfo,

    /// Total memory of the system in MiB.
    pub total_memory: usize,

    /// Whether the CPU supports hardware-accelerated virtualization
    #[serde(deserialize_with = "deserialize_bool_from_int")]
    pub hvm_supported: bool,

    /// Whether the system was booted with SecureBoot enabled
    #[serde(default, deserialize_with = "deserialize_bool_from_int_maybe")]
    pub secure_boot: Option<bool>,
}

#[derive(Copy, Clone, Eq, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BootType {
    Bios,
    Efi,
}

#[derive(Clone, Deserialize)]
pub struct NetworkInfo {
    pub dns: Dns,
    pub routes: Option<Routes>,

    /// Maps devices to their configuration, if it has a usable configuration.
    /// (Contains no entries for devices with only link-local addresses.)
    #[serde(default)]
    pub interfaces: BTreeMap<String, Interface>,

    /// The hostname of this machine, if set by the DHCP server.
    pub hostname: Option<String>,
}

#[derive(Clone, Deserialize)]
pub struct Dns {
    pub domain: Option<String>,

    /// List of stringified IP addresses.
    #[serde(default)]
    pub dns: Vec<IpAddr>,
}

#[derive(Clone, Deserialize)]
pub struct Routes {
    /// Ipv4 gateway.
    pub gateway4: Option<Gateway>,

    /// Ipv6 gateway.
    pub gateway6: Option<Gateway>,
}

#[derive(Clone, Deserialize)]
pub struct Gateway {
    /// Outgoing network device.
    pub dev: String,

    /// Stringified gateway IP address.
    pub gateway: IpAddr,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum InterfaceState {
    Up,
    Down,
    #[serde(other)]
    Unknown,
}

impl InterfaceState {
    // avoid display trait as this is not the string representation for a serializer
    pub fn render(&self) -> String {
        match self {
            Self::Up => "\u{25CF}",
            Self::Down | Self::Unknown => " ",
        }
        .into()
    }
}

#[derive(Clone, Deserialize)]
pub struct Interface {
    pub name: String,

    pub index: usize,

    pub mac: String,

    pub state: InterfaceState,

    #[serde(default)]
    #[serde(deserialize_with = "deserialize_cidr_list")]
    pub addresses: Option<Vec<CidrAddress>>,
}

impl Interface {
    // avoid display trait as this is not the string representation for a serializer
    pub fn render(&self) -> String {
        format!("{} {}", self.state.render(), self.name)
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct InstallRootPassword {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hashed: Option<String>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct InstallFirstBootSetup {
    #[serde(
        serialize_with = "serialize_bool_as_u32",
        deserialize_with = "deserialize_bool_from_int"
    )]
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ordering_target: Option<String>,
}

pub fn spawn_low_level_installer(test_mode: bool) -> io::Result<process::Child> {
    let (path, args, envs): (&str, &[&str], Vec<(&str, &str)>) = if test_mode {
        (
            "./proxmox-low-level-installer",
            &["-t", "/dev/null", "start-session-test"],
            vec![("PERL5LIB", ".")],
        )
    } else {
        ("proxmox-low-level-installer", &["start-session"], vec![])
    };

    Command::new(path)
        .args(args)
        .envs(envs)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
}

/// See Proxmox::Install::Config
#[derive(Deserialize, Serialize)]
pub struct InstallConfig {
    pub autoreboot: usize,

    pub filesys: FsType,
    pub hdsize: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swapsize: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maxroot: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minfree: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maxvz: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub zfs_opts: Option<InstallZfsOption>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub btrfs_opts: Option<InstallBtrfsOption>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_hd: Option<String>,

    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub disk_selection: BTreeMap<String, String>,

    pub existing_storage_auto_rename: usize,

    pub country: String,
    pub timezone: String,
    pub keymap: String,

    pub root_password: InstallRootPassword,
    pub mailto: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub root_ssh_keys: Vec<String>,

    pub mngmt_nic: String,

    pub hostname: String,
    pub domain: String,
    #[serde(serialize_with = "serialize_as_display")]
    pub cidr: CidrAddress,
    pub gateway: IpAddr,
    pub dns: IpAddr,

    pub first_boot: InstallFirstBootSetup,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LowLevelMessage {
    #[serde(rename = "message")]
    Info {
        message: String,
    },
    Error {
        message: String,
    },
    Prompt {
        query: String,
    },
    Finished {
        state: String,
        message: String,
    },
    Progress {
        ratio: f32,
        text: Option<String>,
    },
}
