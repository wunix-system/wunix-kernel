// Смещения регистров контроллера Synopsys DesignWare I2C
const IC_CON: usize = 0x00;
const IC_TAR: usize = 0x04;
const IC_DATA_CMD: usize = 0x10;
const IC_SS_SCL_HCNT: usize = 0x14;
const IC_SS_SCL_LCNT: usize = 0x18;
const IC_FS_SCL_HCNT: usize = 0x1C;
const IC_FS_SCL_LCNT: usize = 0x20;
const IC_INTR_MASK: usize = 0x30;
const IC_CLR_INTR: usize = 0x40;
const IC_ENABLE: usize = 0x6C;
const IC_STATUS: usize = 0x70;
const _IC_TXFLR: usize = 0x74;
const _IC_RXFLR: usize = 0x78;

// Битовые маски IC_CON
const IC_CON_MASTER_MODE: u32 = 1 << 0;
const IC_CON_SPEED_FAST: u32 = 2 << 1; // 400 кГц
const IC_CON_RESTART_EN: u32 = 1 << 5;
const IC_CON_SLAVE_DISABLE: u32 = 1 << 6;

// Битовые маски IC_STATUS
const IC_STATUS_ACTIVITY: u32 = 1 << 0;
const IC_STATUS_TFNF: u32 = 1 << 1; // TX FIFO Not Full
const IC_STATUS_RFNE: u32 = 1 << 3; // RX FIFO Not Empty

pub struct DesignwareI2c {
    base: u64,
}

impl DesignwareI2c {
    pub fn new(mmio_base_virt: u64) -> Self {
        Self {
            base: mmio_base_virt,
        }
    }

    #[inline]
    fn read_reg(&self, offset: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.base + offset as u64) as *const u32) }
    }

    #[inline]
    fn write_reg(&mut self, offset: usize, val: u32) {
        unsafe { core::ptr::write_volatile((self.base + offset as u64) as *mut u32, val) }
    }

    pub fn init(&mut self) -> Result<(), &'static str> {
        // Отключаем контроллер перед конфигурированием
        self.set_enable(false)?;

        // Настраиваем режим мастера: 400 кГц (Fast mode), 7-битная адресация, разрешены повторные START (RESTART)
        let con = IC_CON_MASTER_MODE | IC_CON_SPEED_FAST | IC_CON_RESTART_EN | IC_CON_SLAVE_DISABLE;
        self.write_reg(IC_CON, con);

        // Тайминги SCL для частоты шины 100 МГц (стандартная частота PCH LPSS)
        self.write_reg(IC_FS_SCL_HCNT, 90);
        self.write_reg(IC_FS_SCL_LCNT, 160);
        self.write_reg(IC_SS_SCL_HCNT, 400);
        self.write_reg(IC_SS_SCL_LCNT, 470);

        // Маскируем прерывания (работаем в режиме опроса регистров)
        self.write_reg(IC_INTR_MASK, 0);
        let _ = self.read_reg(IC_CLR_INTR);

        // Включаем контроллер
        self.set_enable(true)?;
        crate::klog!("i2c_designware: initialized Fast-Mode 400kHz");
        Ok(())
    }

    fn set_enable(&mut self, enable: bool) -> Result<(), &'static str> {
        let val = if enable { 1 } else { 0 };
        self.write_reg(IC_ENABLE, val);
        for _ in 0..10_000 {
            let status = self.read_reg(IC_ENABLE);
            if (status & 1) == val {
                return Ok(());
            }
        }
        Err("i2c_designware: timeout toggling enable state")
    }

    pub fn write(&mut self, slave_addr: u8, data: &[u8]) -> Result<(), &'static str> {
        self.set_enable(false)?;
        self.write_reg(IC_TAR, slave_addr as u32);
        self.set_enable(true)?;

        for (i, &byte) in data.iter().enumerate() {
            // Ждем освобождения места в TX FIFO
            self.wait_tx_not_full()?;
            let mut cmd = byte as u32;
            if i == data.len() - 1 {
                cmd |= 1 << 9; // STOP bit
            }
            self.write_reg(IC_DATA_CMD, cmd);
        }

        self.wait_idle()?;
        Ok(())
    }

    pub fn read(&mut self, slave_addr: u8, buf: &mut [u8]) -> Result<(), &'static str> {
        self.set_enable(false)?;
        self.write_reg(IC_TAR, slave_addr as u32);
        self.set_enable(true)?;

        let len = buf.len();
        for i in 0..len {
            self.wait_tx_not_full()?;
            let mut cmd = 1 << 8; // CMD: Read
            if i == len - 1 {
                cmd |= 1 << 9; // STOP bit
            }
            self.write_reg(IC_DATA_CMD, cmd);
        }

        for byte in buf.iter_mut() {
            self.wait_rx_not_empty()?;
            *byte = (self.read_reg(IC_DATA_CMD) & 0xFF) as u8;
        }

        self.wait_idle()?;
        Ok(())
    }

    fn wait_tx_not_full(&self) -> Result<(), &'static str> {
        for _ in 0..100_000 {
            if (self.read_reg(IC_STATUS) & IC_STATUS_TFNF) != 0 {
                return Ok(());
            }
        }
        Err("i2c: TX FIFO full timeout")
    }

    fn wait_rx_not_empty(&self) -> Result<(), &'static str> {
        for _ in 0..100_000 {
            if (self.read_reg(IC_STATUS) & IC_STATUS_RFNE) != 0 {
                return Ok(());
            }
        }
        Err("i2c: RX FIFO empty timeout")
    }

    fn wait_idle(&self) -> Result<(), &'static str> {
        for _ in 0..100_000 {
            if (self.read_reg(IC_STATUS) & IC_STATUS_ACTIVITY) == 0 {
                return Ok(());
            }
        }
        Err("i2c: bus busy timeout")
    }
}
