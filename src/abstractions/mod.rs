pub mod imodel;
pub mod irepository;
pub mod icontroller;
pub mod iservice;
pub mod idto;
pub mod iconfiguration;
pub mod iutility;

pub use imodel::IModel;
pub use irepository::IRepository;
pub use icontroller::IController;
pub use iservice::{IService, ServiceResult};
pub use idto::{IRequestDto, IResponseDto};
pub use iconfiguration::IConfiguration;
pub use iutility::IUtility;
