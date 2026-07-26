use serde::{Serialize, Deserialize};
#[cfg(feature = "utoipa")]
use utoipa::ToSchema;
use chrono::{DateTime, Utc};
use uuid::Uuid;
use redb::{Database, TableDefinition, ReadableTable};
use anyhow::{Result, Context};
use std::path::Path;
use chrono::Duration as ChronoDuration;

// Table Definitions
const MODELS: TableDefinition<&str, &str> = TableDefinition::new("models");
const SEQUENCES: TableDefinition<&str, &str> = TableDefinition::new("sequences");
const SEQUENCE_LEASES: TableDefinition<&str, &str> = TableDefinition::new("sequence_leases");
const SEQUENCE_RUNTIME: TableDefinition<&str, &str> = TableDefinition::new("sequence_runtime");
const CAMERA_CALIBRATIONS: TableDefinition<&str, &str> = TableDefinition::new("camera_calibrations");
const CAMERA_COLOR_CALIBRATIONS: TableDefinition<&str, &str> =
    TableDefinition::new("camera_color_calibrations");
const MACHINES: TableDefinition<&str, &str> = TableDefinition::new("machines");
const DEVICES: TableDefinition<&str, &str> = TableDefinition::new("devices");

#[derive(Debug, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
pub struct Model {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub notes: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub thumbnail_path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Sequence {
    pub id: Uuid,
    pub model_id: Uuid,
    pub name: String,
    pub camera_preset: String, 
    pub folder_path: String,
    pub status: SequenceStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum SequenceStatus {
    Planned,
    Capturing,
    Processing,
    Completed,
    Failed,
    Archived,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SequenceLease {
    pub sequence_id: Uuid,
    pub owner_id: Uuid,
    pub owner_name: String,
    pub acquired_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct SequenceRuntime {
    pub sequence_id: Uuid,
    pub failure_count: u32,
    pub last_error: Option<String>,
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Machine {
    pub id: Uuid,
    pub name: String,
    pub hostname: String,
    pub ip_address: Option<String>,
    pub status: String, // "Online", "Offline"
    pub last_seen: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Device {
    pub id: Uuid,
    pub machine_id: Uuid,
    pub name: String,
    pub device_type: DeviceType,
    pub connection_string: String, // e.g., "usb:001", "ble:MAC"
    pub config_json: String,       // Arbitrary JSON config
    pub enabled: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum DeviceType {
    Camera,
    Turntable,
    Light,
    RobotArm,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CameraCalibration {
    pub camera_id: String,
    pub camera_matrix: Vec<f64>,
    pub distortion: Vec<f64>,
    pub rms_error: f64,
    pub width: i32,
    pub height: i32,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CameraColorCalibration {
    pub camera_id: String,
    pub ccm: [[f32; 3]; 3],
    pub delta_e: f32,
    pub updated_at: DateTime<Utc>,
}

pub struct Inventory {
    db: Database,
}

impl Inventory {
    pub fn new(path: &Path) -> Result<Self> {
        let db = Database::create(path).context("Failed to open inventory DB")?;
        
        // Initialize tables if needed
        let write_txn = db.begin_write()?;
        {
            let _ = write_txn.open_table(MODELS)?;
            let _ = write_txn.open_table(SEQUENCES)?;
            let _ = write_txn.open_table(SEQUENCE_LEASES)?;
            let _ = write_txn.open_table(SEQUENCE_RUNTIME)?;
            let _ = write_txn.open_table(CAMERA_CALIBRATIONS)?;
            let _ = write_txn.open_table(CAMERA_COLOR_CALIBRATIONS)?;
            let _ = write_txn.open_table(MACHINES)?;
            let _ = write_txn.open_table(DEVICES)?;
        }
        write_txn.commit()?;
        
        Ok(Self { db })
    }
    
    // --- Models ---
    
    pub fn create_model(&self, name: &str, description: &str) -> Result<Model> {
        let model = Model {
            id: Uuid::new_v4(),
            name: name.to_string(),
            description: description.to_string(),
            notes: String::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            thumbnail_path: None,
        };
        
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(MODELS)?;
            let json = serde_json::to_string(&model)?;
            let id_str = model.id.to_string();
            table.insert(id_str.as_str(), json.as_str())?;
        }
        write_txn.commit()?;
        
        Ok(model)
    }
    
    pub fn get_model(&self, id: &Uuid) -> Result<Option<Model>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(MODELS)?;
        let id_str = id.to_string();
        let result = table.get(id_str.as_str())?;
        
        if let Some(val) = result {
            let model: Model = serde_json::from_str(val.value())?;
            Ok(Some(model))
        } else {
            Ok(None)
        }
    }
    
    pub fn list_models(&self) -> Result<Vec<Model>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(MODELS)?;
        let mut models = Vec::new();
        for item in table.iter()? {
            let (_, val) = item?;
            let model: Model = serde_json::from_str(val.value())?;
            models.push(model);
        }
        Ok(models)
    }

    pub fn delete_model(&self, id: &Uuid) -> Result<bool> {
        let write_txn = self.db.begin_write()?;
        let id_str = id.to_string();
        let removed = {
            let mut model_table = write_txn.open_table(MODELS)?;
            let removed = model_table.remove(id_str.as_str())?;
            removed.is_some()
        };

        if removed {
            let mut seq_table = write_txn.open_table(SEQUENCES)?;
            let mut to_delete: Vec<String> = Vec::new();
            for item in seq_table.iter()? {
                let (key, val) = item?;
                let seq: Sequence = serde_json::from_str(val.value())?;
                if seq.model_id == *id {
                    to_delete.push(key.value().to_string());
                }
            }
            for seq_id in to_delete {
                let _ = seq_table.remove(seq_id.as_str())?;
            }
        }

        write_txn.commit()?;
        Ok(removed)
    }

    pub fn touch_model(&self, id: &Uuid) -> Result<Option<Model>> {
        let write_txn = self.db.begin_write()?;
        let updated = {
            let mut table = write_txn.open_table(MODELS)?;
            let id_str = id.to_string();
            let (json, model) = {
                let existing = table.get(id_str.as_str())?;
                if let Some(val) = existing {
                    let mut model: Model = serde_json::from_str(val.value())?;
                    model.updated_at = Utc::now();
                    let json = serde_json::to_string(&model)?;
                    (Some(json), Some(model))
                } else {
                    (None, None)
                }
            };
            if let Some(json) = json {
                table.insert(id_str.as_str(), json.as_str())?;
            }
            model
        };
        write_txn.commit()?;
        Ok(updated)
    }

    // --- Sequences ---
    
    pub fn create_sequence(&self, model_id: Uuid, name: &str) -> Result<Sequence> {
         let seq = Sequence {
            id: Uuid::new_v4(),
            model_id,
            name: name.to_string(),
            camera_preset: "Default".to_string(),
            folder_path: "".to_string(),
            status: SequenceStatus::Planned,
            created_at: Utc::now(),
        };
        
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(SEQUENCES)?;
            let json = serde_json::to_string(&seq)?;
            let id_str = seq.id.to_string();
            table.insert(id_str.as_str(), json.as_str())?;
        }
        write_txn.commit()?;
        
        Ok(seq)
    }

    pub fn update_sequence_status(&self, id: &Uuid, status: SequenceStatus) -> Result<Option<Sequence>> {
        let write_txn = self.db.begin_write()?;
        let updated = {
            let mut table = write_txn.open_table(SEQUENCES)?;
            let id_str = id.to_string();
            let (json, seq) = {
                let existing = table.get(id_str.as_str())?;
                if let Some(val) = existing {
                    let mut seq: Sequence = serde_json::from_str(val.value())?;
                    seq.status = status;
                    let json = serde_json::to_string(&seq)?;
                    (Some(json), Some(seq))
                } else {
                    (None, None)
                }
            };
            if let Some(json) = json {
                table.insert(id_str.as_str(), json.as_str())?;
            }
            seq
        };
        write_txn.commit()?;
        Ok(updated)
    }

    pub fn transition_sequence_status(
        &self,
        id: &Uuid,
        expected: SequenceStatus,
        next: SequenceStatus,
    ) -> Result<Option<Sequence>> {
        let write_txn = self.db.begin_write()?;
        let updated = {
            let mut table = write_txn.open_table(SEQUENCES)?;
            let id_str = id.to_string();
            let (json, seq) = {
                let existing = table.get(id_str.as_str())?;
                if let Some(val) = existing {
                    let mut seq: Sequence = serde_json::from_str(val.value())?;
                    if seq.status != expected {
                        return Ok(None);
                    }
                    seq.status = next;
                    let json = serde_json::to_string(&seq)?;
                    (Some(json), Some(seq))
                } else {
                    (None, None)
                }
            };
            if let Some(json) = json {
                table.insert(id_str.as_str(), json.as_str())?;
            }
            seq
        };
        write_txn.commit()?;
        Ok(updated)
    }

    pub fn try_acquire_sequence_lease(
        &self,
        id: &Uuid,
        owner_id: &Uuid,
        owner_name: &str,
        ttl: ChronoDuration,
    ) -> Result<bool> {
        let now = Utc::now();
        let write_txn = self.db.begin_write()?;
        let acquired = {
            let mut table = write_txn.open_table(SEQUENCE_LEASES)?;
            let id_str = id.to_string();
            let existing = table
                .get(id_str.as_str())?
                .map(|val| serde_json::from_str::<SequenceLease>(val.value()))
                .transpose()?;
            if let Some(lease) = existing {
                if lease.expires_at > now && lease.owner_id != *owner_id {
                    return Ok(false);
                }
            }
            let lease = SequenceLease {
                sequence_id: *id,
                owner_id: *owner_id,
                owner_name: owner_name.to_string(),
                acquired_at: now,
                expires_at: now + ttl,
            };
            let json = serde_json::to_string(&lease)?;
            table.insert(id_str.as_str(), json.as_str())?;
            true
        };
        write_txn.commit()?;
        Ok(acquired)
    }

    pub fn renew_sequence_lease(
        &self,
        id: &Uuid,
        owner_id: &Uuid,
        ttl: ChronoDuration,
    ) -> Result<bool> {
        let now = Utc::now();
        let write_txn = self.db.begin_write()?;
        let renewed = {
            let mut table = write_txn.open_table(SEQUENCE_LEASES)?;
            let id_str = id.to_string();
            let existing = table
                .get(id_str.as_str())?
                .map(|val| serde_json::from_str::<SequenceLease>(val.value()))
                .transpose()?;
            if let Some(mut lease) = existing {
                if lease.owner_id != *owner_id {
                    return Ok(false);
                }
                lease.expires_at = now + ttl;
                let json = serde_json::to_string(&lease)?;
                table.insert(id_str.as_str(), json.as_str())?;
                true
            } else {
                false
            }
        };
        write_txn.commit()?;
        Ok(renewed)
    }

    pub fn release_sequence_lease(&self, id: &Uuid, owner_id: &Uuid) -> Result<bool> {
        let write_txn = self.db.begin_write()?;
        let removed = {
            let mut table = write_txn.open_table(SEQUENCE_LEASES)?;
            let id_str = id.to_string();
            let existing = table
                .get(id_str.as_str())?
                .map(|val| serde_json::from_str::<SequenceLease>(val.value()))
                .transpose()?;
            if let Some(lease) = existing {
                if lease.owner_id != *owner_id {
                    return Ok(false);
                }
                table.remove(id_str.as_str())?.is_some()
            } else {
                false
            }
        };
        write_txn.commit()?;
        Ok(removed)
    }

    pub fn get_sequence_lease(&self, id: &Uuid) -> Result<Option<SequenceLease>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(SEQUENCE_LEASES)?;
        let id_str = id.to_string();
        let existing = table.get(id_str.as_str())?;
        if let Some(val) = existing {
            let lease: SequenceLease = serde_json::from_str(val.value())?;
            Ok(Some(lease))
        } else {
            Ok(None)
        }
    }

    pub fn record_sequence_failure(&self, id: &Uuid, error: &str) -> Result<SequenceRuntime> {
        let write_txn = self.db.begin_write()?;
        let updated = {
            let mut table = write_txn.open_table(SEQUENCE_RUNTIME)?;
            let id_str = id.to_string();
            let mut runtime = if let Some(val) = table.get(id_str.as_str())? {
                serde_json::from_str::<SequenceRuntime>(val.value())?
            } else {
                SequenceRuntime {
                    sequence_id: *id,
                    ..Default::default()
                }
            };
            runtime.failure_count = runtime.failure_count.saturating_add(1);
            runtime.last_error = Some(error.to_string());
            runtime.last_attempt_at = Some(Utc::now());
            let json = serde_json::to_string(&runtime)?;
            table.insert(id_str.as_str(), json.as_str())?;
            runtime
        };
        write_txn.commit()?;
        Ok(updated)
    }

    pub fn record_sequence_success(&self, id: &Uuid) -> Result<SequenceRuntime> {
        let write_txn = self.db.begin_write()?;
        let updated = {
            let mut table = write_txn.open_table(SEQUENCE_RUNTIME)?;
            let id_str = id.to_string();
            let mut runtime = if let Some(val) = table.get(id_str.as_str())? {
                serde_json::from_str::<SequenceRuntime>(val.value())?
            } else {
                SequenceRuntime {
                    sequence_id: *id,
                    ..Default::default()
                }
            };
            runtime.last_success_at = Some(Utc::now());
            runtime.last_error = None;
            runtime.last_attempt_at = Some(Utc::now());
            let json = serde_json::to_string(&runtime)?;
            table.insert(id_str.as_str(), json.as_str())?;
            runtime
        };
        write_txn.commit()?;
        Ok(updated)
    }

    pub fn get_sequence_runtime(&self, id: &Uuid) -> Result<Option<SequenceRuntime>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(SEQUENCE_RUNTIME)?;
        let id_str = id.to_string();
        let existing = table.get(id_str.as_str())?;
        if let Some(val) = existing {
            let runtime: SequenceRuntime = serde_json::from_str(val.value())?;
            Ok(Some(runtime))
        } else {
            Ok(None)
        }
    }

    pub fn upsert_camera_calibration(
        &self,
        camera_id: &str,
        camera_matrix: Vec<f64>,
        distortion: Vec<f64>,
        rms_error: f64,
        width: i32,
        height: i32,
    ) -> Result<CameraCalibration> {
        let write_txn = self.db.begin_write()?;
        let calibration = {
            let mut table = write_txn.open_table(CAMERA_CALIBRATIONS)?;
            let calibration = CameraCalibration {
                camera_id: camera_id.to_string(),
                camera_matrix,
                distortion,
                rms_error,
                width,
                height,
                updated_at: Utc::now(),
            };
            let json = serde_json::to_string(&calibration)?;
            table.insert(camera_id, json.as_str())?;
            calibration
        };
        write_txn.commit()?;
        Ok(calibration)
    }

    pub fn upsert_camera_color_calibration(
        &self,
        camera_id: &str,
        ccm: [[f32; 3]; 3],
        delta_e: f32,
    ) -> Result<CameraColorCalibration> {
        let write_txn = self.db.begin_write()?;
        let calibration = {
            let mut table = write_txn.open_table(CAMERA_COLOR_CALIBRATIONS)?;
            let calibration = CameraColorCalibration {
                camera_id: camera_id.to_string(),
                ccm,
                delta_e,
                updated_at: Utc::now(),
            };
            let json = serde_json::to_string(&calibration)?;
            table.insert(camera_id, json.as_str())?;
            calibration
        };
        write_txn.commit()?;
        Ok(calibration)
    }

    pub fn get_camera_calibration(&self, camera_id: &str) -> Result<Option<CameraCalibration>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(CAMERA_CALIBRATIONS)?;
        let existing = table.get(camera_id)?;
        if let Some(val) = existing {
            let calibration: CameraCalibration = serde_json::from_str(val.value())?;
            Ok(Some(calibration))
        } else {
            Ok(None)
        }
    }

    pub fn get_camera_color_calibration(
        &self,
        camera_id: &str,
    ) -> Result<Option<CameraColorCalibration>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(CAMERA_COLOR_CALIBRATIONS)?;
        let existing = table.get(camera_id)?;
        if let Some(val) = existing {
            let calibration: CameraColorCalibration = serde_json::from_str(val.value())?;
            Ok(Some(calibration))
        } else {
            Ok(None)
        }
    }

    pub fn update_sequence_folder(&self, id: &Uuid, folder_path: &str) -> Result<Option<Sequence>> {
        let write_txn = self.db.begin_write()?;
        let updated = {
            let mut table = write_txn.open_table(SEQUENCES)?;
            let id_str = id.to_string();
            let (json, seq) = {
                let existing = table.get(id_str.as_str())?;
                if let Some(val) = existing {
                    let mut seq: Sequence = serde_json::from_str(val.value())?;
                    seq.folder_path = folder_path.to_string();
                    let json = serde_json::to_string(&seq)?;
                    (Some(json), Some(seq))
                } else {
                    (None, None)
                }
            };
            if let Some(json) = json {
                table.insert(id_str.as_str(), json.as_str())?;
            }
            seq
        };
        write_txn.commit()?;
        Ok(updated)
    }
    
    pub fn list_sequences_for_model(&self, model_id: &Uuid) -> Result<Vec<Sequence>> {
         let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(SEQUENCES)?;
        let mut seqs = Vec::new();
        for item in table.iter()? {
            let (_, val) = item?;
            let seq: Sequence = serde_json::from_str(val.value())?;
            if seq.model_id == *model_id {
                seqs.push(seq);
            }
        }
        Ok(seqs)
    }

    pub fn list_sequences(&self) -> Result<Vec<Sequence>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(SEQUENCES)?;
        let mut seqs = Vec::new();
        for item in table.iter()? {
            let (_, val) = item?;
            let seq: Sequence = serde_json::from_str(val.value())?;
            seqs.push(seq);
        }
        Ok(seqs)
    }

    pub fn list_sequences_by_status(&self, status: SequenceStatus) -> Result<Vec<Sequence>> {
        let sequences = self.list_sequences()?;
        Ok(sequences
            .into_iter()
            .filter(|seq| seq.status == status)
            .collect())
    }

    // --- Machines ---

    pub fn register_machine(&self, name: &str, hostname: &str) -> Result<Machine> {
        let machine = Machine {
            id: Uuid::new_v4(),
            name: name.to_string(),
            hostname: hostname.to_string(),
            ip_address: None,
            status: "Online".to_string(),
            last_seen: Utc::now(),
        };
        
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(MACHINES)?;
            let json = serde_json::to_string(&machine)?;
            table.insert(machine.id.to_string().as_str(), json.as_str())?;
        }
        write_txn.commit()?;
        Ok(machine)
    }

    pub fn list_machines(&self) -> Result<Vec<Machine>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(MACHINES)?;
        let mut list = Vec::new();
        for item in table.iter()? {
            let (_, val) = item?;
            list.push(serde_json::from_str(val.value())?);
        }
        Ok(list)
    }

    // --- Devices ---

    pub fn register_device(&self, machine_id: Uuid, name: &str, device_type: DeviceType, connection: &str) -> Result<Device> {
        let device = Device {
            id: Uuid::new_v4(),
            machine_id,
            name: name.to_string(),
            device_type,
            connection_string: connection.to_string(),
            config_json: "{}".to_string(),
            enabled: true,
        };
        
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(DEVICES)?;
            let json = serde_json::to_string(&device)?;
            table.insert(device.id.to_string().as_str(), json.as_str())?;
        }
        write_txn.commit()?;
        Ok(device)
    }

    pub fn list_devices_for_machine(&self, machine_id: &Uuid) -> Result<Vec<Device>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(DEVICES)?;
        let mut list = Vec::new();
        for item in table.iter()? {
            let (_, val) = item?;
            let dev: Device = serde_json::from_str(val.value())?;
            if dev.machine_id == *machine_id {
                list.push(dev);
            }
        }
        Ok(list)
    }
}
