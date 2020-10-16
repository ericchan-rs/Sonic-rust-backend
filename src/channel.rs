use crate::commands::*;
use crate::result::*;
use std::fmt;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::net::{TcpStream, ToSocketAddrs};

const DEFAULT_SONIC_PROTOCOL_VERSION: usize = 1;
const UNINITIALIZED_MODE_MAX_BUFFER_SIZE: usize = 200;

macro_rules! init_commands {
    (
        $(
            $(#[$outer:meta])*
            use $cmd_name:ident
            for fn $fn_name:ident
            $(<$($lt:lifetime)+>)? (
                $($args:tt)*
            )
            $(where mode is $condition:expr)?;
        )*
    ) => {
        $(
            init_commands!(
                $(#[$outer])*
                use $cmd_name
                for fn $fn_name $(<$($lt)+>)? (
                    $($args)*
                )
                $(where $condition)?
            );
        )*
    };

    (
        $(#[$outer:meta])*
        use $cmd_name:ident
        for fn $fn_name:ident $(<$($lt:lifetime)+>)? (
            $($arg_name:ident : $arg_type:ty $( => $arg_value:expr)?,)*
        )
        $(where $condition:expr)?
    ) => {
        $(#[$outer])*
        pub fn $fn_name $(<$($lt)+>)? (
            &self,
            $($arg_name: $arg_type),*
        ) -> $crate::result::Result<
            <$cmd_name as $crate::commands::StreamCommand>::Response,
        > {
            $(
                let mode = self.mode.clone();
                if mode != Some($condition) {
                    return Err(Error::new(
                        ErrorKind::UnsupportedCommand((
                            stringify!($fn_name), 
                            mode,
                        ))
                    ));
                }
            )?
            #[allow(clippy::needless_update)]
            let command = $cmd_name { $($arg_name $(: $arg_value)?,)* ..Default::default() };
            self.run_command(command)
        }
    };
}

/// Channel modes supported by sonic search backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChannelMode {
    /// Sonic server search channel mode.
    ///
    /// In this mode you can use `query`, `suggest`, `ping` and `quit` commands.
    ///
    /// Note: This mode requires enabling the `search` feature.
    #[cfg(feature = "search")]
    Search,

    /// Sonic server ingest channel mode.
    ///
    /// In this mode you can use `push`, `pop`, `flushc`, `flushb`, `flusho`,
    /// `bucket_count`, `object_count`, `word_count`, `ping` and `quit` commands.
    ///
    /// Note: This mode requires enabling the `ingest` feature.
    #[cfg(feature = "ingest")]
    Ingest,

    /// Sonic server control channel mode.
    ///
    /// In this mode you can use `consolidate`, `backup`, `restore`,
    /// `ping` and `quit` commands.
    ///
    /// Note: This mode requires enabling the `control` feature.
    #[cfg(feature = "control")]
    Control,
}

impl ChannelMode {
    /// Converts enum to &str
    pub fn to_str(&self) -> &str {
        match self {
            #[cfg(feature = "search")]
            ChannelMode::Search => "search",

            #[cfg(feature = "ingest")]
            ChannelMode::Ingest => "ingest",

            #[cfg(feature = "control")]
            ChannelMode::Control => "control",
        }
    }
}

impl fmt::Display for ChannelMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::result::Result<(), fmt::Error> {
        write!(f, "{}", self.to_str())
    }
}

/// Root and Heart of this library.
///
/// You can connect to the sonic search backend and run all supported protocol methods.
///
#[derive(Debug)]
pub struct SonicChannel {
    stream: TcpStream,
    mode: Option<ChannelMode>, // None – Uninitialized mode
    max_buffer_size: usize,
    protocol_version: usize,
}

impl SonicChannel {
    fn write<SC: StreamCommand>(&self, command: &SC) -> Result<()> {
        let mut writer = BufWriter::with_capacity(self.max_buffer_size, &self.stream);
        let message = command.message();
        dbg!(&message);
        writer
            .write_all(message.as_bytes())
            .map_err(|_| Error::new(ErrorKind::WriteToStream))?;
        Ok(())
    }

    fn read(&self, max_read_lines: usize) -> Result<String> {
        let mut reader = BufReader::with_capacity(self.max_buffer_size, &self.stream);
        let mut message = String::new();

        let mut lines_read = 0;
        while lines_read < max_read_lines {
            reader
                .read_line(&mut message)
                .map_err(|_| Error::new(ErrorKind::ReadStream))?;
            lines_read += 1;
        }

        Ok(message)
    }

    fn run_command<SC: StreamCommand>(&self, command: SC) -> Result<SC::Response> {
        self.write(&command)?;
        let message = self.read(SC::READ_LINES_COUNT)?;
        command.receive(message)
    }

    fn connect<A: ToSocketAddrs>(addr: A) -> Result<Self> {
        let stream =
            TcpStream::connect(addr).map_err(|_| Error::new(ErrorKind::ConnectToServer))?;

        let channel = SonicChannel {
            stream,
            mode: None,
            max_buffer_size: UNINITIALIZED_MODE_MAX_BUFFER_SIZE,
            protocol_version: DEFAULT_SONIC_PROTOCOL_VERSION,
        };

        let message = channel.read(1)?;
        dbg!(&message);
        // TODO: need to add support for versions
        if message.starts_with("CONNECTED") {
            Ok(channel)
        } else {
            Err(Error::new(ErrorKind::ConnectToServer))
        }
    }

    fn start<S: ToString>(&mut self, mode: ChannelMode, password: S) -> Result<()> {
        if self.mode.is_some() {
            return Err(Error::new(ErrorKind::RunCommand));
        }

        let command = StartCommand {
            mode,
            password: password.to_string(),
        };
        let response = self.run_command(command)?;

        self.max_buffer_size = response.max_buffer_size;
        self.protocol_version = response.protocol_version;
        self.mode = Some(response.mode);

        Ok(())
    }

    /// Connect to the search backend in chosen mode.
    ///
    /// I think we shouldn't separate commands connect and start because we haven't
    /// possibility to change channel in sonic server, if we already chosen one of them. 🤔
    ///
    /// ```rust,no_run
    /// use sonic_channel::*;
    ///
    /// fn main() -> result::Result<()> {
    ///     let channel = SonicChannel::connect_with_start(
    ///         ChannelMode::Search,
    ///         "localhost:1491",
    ///         "SecretPassword"
    ///     )?;
    ///
    ///     // Now you can use all method of Search channel.
    ///     let objects = channel.query("search", "default", "beef");
    ///     
    ///     Ok(())
    /// }
    /// ```
    pub fn connect_with_start<A, S>(mode: ChannelMode, addr: A, password: S) -> Result<Self>
    where
        A: ToSocketAddrs,
        S: ToString,
    {
        let mut channel = Self::connect(addr)?;
        channel.start(mode, password)?;
        Ok(channel)
    }

    init_commands! {
        #[doc=r#"
        Stop connection.

        ```rust,no_run
        # use sonic_channel::*;
        # fn main() -> result::Result<()> {
        let channel = SonicChannel::connect_with_start(
            ChannelMode::Search,
            "localhost:1491",
            "SecretPassword",
        )?;

        channel.quit()?;
        # Ok(())
        # }
        "#]
        use QuitCommand for fn quit();

        #[doc=r#"
        Ping server.

        ```rust,no_run
        # use sonic_channel::*;
        # fn main() -> result::Result<()> {
        let channel = SonicChannel::connect_with_start(
            ChannelMode::Search,
            "localhost:1491",
            "SecretPassword",
        )?;

        channel.ping()?;
        # Ok(())
        # }
        "#]
        use PingCommand for fn ping();
    }

    #[cfg(feature = "ingest")]
    init_commands! {
        #[doc=r#"
        Push search data in the index.

        Note: This method requires enabling the `ingest` feature and start
        connection in Ingest mode.

        ```rust,no_run
        # use sonic_channel::*;
        # fn main() -> result::Result<()> {
        let ingest_channel = SonicChannel::connect_with_start(
            ChannelMode::Ingest,
            "localhost:1491",
            "SecretPassword",
        )?;

        let result = ingest_channel.push(
            "search",
            "default",
            "recipe:295",
            "Sweet Teriyaki Beef Skewers",
        )?;
        assert_eq!(result, true);
        # Ok(())
        # }
        ```
        "#]
        use PushCommand for fn push<'a>(
            collection: &'a str,
            bucket: &'a str,
            object: &'a str,
            text: &'a str,
        ) where mode is ChannelMode::Ingest;

        #[doc=r#"
        Push search data in the index with locale parameter in ISO 639-3 code.

        Note: This method requires enabling the `ingest` feature and start 
        connection in Ingest mode.

        ```rust,no_run
        # use sonic_channel::*;
        # fn main() -> result::Result<()> {
        let ingest_channel = SonicChannel::connect_with_start(
            ChannelMode::Ingest,
            "localhost:1491",
            "SecretPassword",
        )?;

        let result = ingest_channel.push_with_locale(
            "search",
            "default",
            "recipe:296",
            "Гренки с жареным картофелем и сыром",
            "rus",
        )?;
        assert_eq!(result, true);
        # Ok(())
        # }
        ```
        "#]
        use PushCommand for fn push_with_locale<'a>(
            collection: &'a str,
            bucket: &'a str,
            object: &'a str,
            text: &'a str,
            locale: &'a str => Some(locale),
        ) where mode is ChannelMode::Ingest;

        #[doc=r#"
        Pop search data from the index. Returns removed words count as usize type.

        Note: This method requires enabling the `ingest` feature and start 
        connection in Ingest mode.

        ```rust,no_run
        # use sonic_channel::*;
        # fn main() -> result::Result<()> {
        let ingest_channel = SonicChannel::connect_with_start(
            ChannelMode::Ingest,
            "localhost:1491",
            "SecretPassword",
        )?;

        let result = ingest_channel.pop("search", "default", "recipe:295", "beef")?;
        assert_eq!(result, 1);
        # Ok(())
        # }
        ```
        "#]
        use PopCommand for fn pop<'a>(
            collection: &'a str,
            bucket: &'a str,
            object: &'a str,
            text: &'a str,
        ) where mode is ChannelMode::Ingest;

        #[doc=r#"
        Flush all indexed data from collections.

        Note: This method requires enabling the `ingest` feature and start 
        connection in Ingest mode.

        ```rust,no_run
        # use sonic_channel::*;
        # fn main() -> result::Result<()> {
        let ingest_channel = SonicChannel::connect_with_start(
            ChannelMode::Ingest,
            "localhost:1491",
            "SecretPassword",
        )?;

        let flushc_count = ingest_channel.flushc("search")?;
        dbg!(flushc_count);
        # Ok(())
        # }
        ```
        "#]
        use FlushCommand for fn flushc<'a>(
            collection: &'a str,
        ) where mode is ChannelMode::Ingest;

        #[doc=r#"
        Flush all indexed data from bucket in a collection.

        Note: This method requires enabling the `ingest` feature and start 
        connection in Ingest mode.

        ```rust,no_run
        # use sonic_channel::*;
        # fn main() -> result::Result<()> {
        let ingest_channel = SonicChannel::connect_with_start(
            ChannelMode::Ingest,
            "localhost:1491",
            "SecretPassword",
        )?;

        let flushb_count = ingest_channel.flushb("search", "default")?;
        dbg!(flushb_count);
        # Ok(())
        # }
        ```
        "#]
        use FlushCommand for fn flushb<'a>(
            collection: &'a str,
            bucket: &'a str => Some(bucket),
        ) where mode is ChannelMode::Ingest;

        #[doc=r#"
        Flush all indexed data from an object in a bucket in collection.

        Note: This method requires enabling the `ingest` feature and start 
        connection in Ingest mode.

        ```rust,no_run
        # use sonic_channel::*;
        # fn main() -> result::Result<()> {
        let ingest_channel = SonicChannel::connect_with_start(
            ChannelMode::Ingest,
            "localhost:1491",
            "SecretPassword",
        )?;

        let flusho_count = ingest_channel.flusho("search", "default", "recipe:296")?;
        dbg!(flusho_count);
        # Ok(())
        # }
        ```
        "#]
        use FlushCommand for fn flusho<'a>(
            collection: &'a str,
            bucket: &'a str => Some(bucket),
            object: &'a str => Some(object),
        ) where mode is ChannelMode::Ingest;

        #[doc=r#"
        Bucket count in indexed search data of your collection.

        Note: This method requires enabling the `ingest` feature and start 
        connection in Ingest mode.

        ```rust,no_run
        # use sonic_channel::*;
        # fn main() -> result::Result<()> {
        let ingest_channel = SonicChannel::connect_with_start(
            ChannelMode::Ingest,
            "localhost:1491",
            "SecretPassword",
        )?;

        let bucket_count = ingest_channel.bucket_count("search")?;
        dbg!(bucket_count);
        # Ok(())
        # }
        ```
        "#]
        use CountCommand for fn bucket_count<'a>(
            collection: &'a str,
        ) where mode is ChannelMode::Ingest;

        #[doc=r#"
        Object count of bucket in indexed search data.

        Note: This method requires enabling the `ingest` feature and start 
        connection in Ingest mode.

        ```rust,no_run
        # use sonic_channel::*;
        # fn main() -> result::Result<()> {
        let ingest_channel = SonicChannel::connect_with_start(
            ChannelMode::Ingest,
            "localhost:1491",
            "SecretPassword",
        )?;

        let object_count = ingest_channel.object_count("search", "default")?;
        dbg!(object_count);
        # Ok(())
        # }
        ```
        "#]
        use CountCommand for fn object_count<'a>(
            collection: &'a str,
            bucket: &'a str => Some(bucket),
        ) where mode is ChannelMode::Ingest;

        #[doc=r#"
        Object word count in indexed bucket search data.

        Note: This method requires enabling the `ingest` feature and start 
        connection in Ingest mode.

        ```rust,no_run
        # use sonic_channel::*;
        # fn main() -> result::Result<()> {
        let ingest_channel = SonicChannel::connect_with_start(
            ChannelMode::Ingest,
            "localhost:1491",
            "SecretPassword",
        )?;

        let word_count = ingest_channel.word_count("search", "default", "recipe:296")?;
        dbg!(word_count);
        # Ok(())
        # }
        ```
        "#]
        use CountCommand for fn word_count<'a>(
            collection: &'a str,
            bucket: &'a str => Some(bucket),
            object: &'a str => Some(object),
        ) where mode is ChannelMode::Ingest;
    }

    #[cfg(feature = "search")]
    init_commands! {
        #[doc=r#"
        Query objects in database.

        Note: This method requires enabling the `search` feature and start 
        connection in Search mode.

        ```rust,no_run
        # use sonic_channel::*;
        # fn main() -> result::Result<()> {
        let search_channel = SonicChannel::connect_with_start(
            ChannelMode::Search,
            "localhost:1491",
            "SecretPassword",
        )?;

        let result = search_channel.query("search", "default", "Beef")?;
        dbg!(result);
        # Ok(())
        # }
        ```
        "#]
        use QueryCommand for fn query<'a>(
            collection: &'a str,
            bucket: &'a str,
            terms: &'a str,
        ) where mode is ChannelMode::Search;

        #[doc=r#"
        Query limited objects in database. This method similar query but
        you can configure limit of result.

        Note: This method requires enabling the `search` feature and start 
        connection in Search mode.

        ```rust,no_run
        # use sonic_channel::*;
        # fn main() -> result::Result<()> {
        let search_channel = SonicChannel::connect_with_start(
            ChannelMode::Search,
            "localhost:1491",
            "SecretPassword",
        )?;

        let result = search_channel.query_with_limit(
            "search",
            "default",
            "Beef",
            10,
        )?;
        dbg!(result);
        # Ok(())
        # }
        ```
        "#]
        use QueryCommand for fn query_with_limit<'a>(
            collection: &'a str,
            bucket: &'a str,
            terms: &'a str,
            limit: usize => Some(limit),
        ) where mode is ChannelMode::Search;

        #[doc=r#"
        Query limited objects in database. This method similar 
        query_with_limit but you can put offset in your query.

        Note: This method requires enabling the `search` feature and start 
        connection in Search mode.

        ```rust,no_run
        # use sonic_channel::*;
        # fn main() -> result::Result<()> {
        let search_channel = SonicChannel::connect_with_start(
            ChannelMode::Search,
            "localhost:1491",
            "SecretPassword",
        )?;

        let result = search_channel.query_with_limit_and_offset(
            "search",
            "default",
            "Beef",
            10,
            10,
        )?;
        dbg!(result);
        # Ok(())
        # }
        ```
        "#]
        use QueryCommand for fn query_with_limit_and_offset<'a>(
            collection: &'a str,
            bucket: &'a str,
            terms: &'a str,
            limit: usize => Some(limit),
            offset: usize => Some(offset),
        ) where mode is ChannelMode::Search;

        #[doc=r#"
        Suggest auto-completes words.

        Note: This method requires enabling the `search` feature and start 
        connection in Search mode.

        ```rust,no_run
        # use sonic_channel::*;
        # fn main() -> result::Result<()> {
        let search_channel = SonicChannel::connect_with_start(
            ChannelMode::Search,
            "localhost:1491",
            "SecretPassword",
        )?;

        let result = search_channel.suggest("search", "default", "Beef")?;
        dbg!(result);
        # Ok(())
        # }
        ```
        "#]
        use SuggestCommand for fn suggest<'a>(
            collection: &'a str,
            bucket: &'a str,
            word: &'a str,
        ) where mode is ChannelMode::Search;

        #[doc=r#"
        Suggest auto-completes words with limit.

        Note: This method requires enabling the `search` feature and start 
        connection in Search mode.

        ```rust,no_run
        # use sonic_channel::*;
        # fn main() -> result::Result<()> {
        let search_channel = SonicChannel::connect_with_start(
            ChannelMode::Search,
            "localhost:1491",
            "SecretPassword",
        )?;

        let result = search_channel.suggest_with_limit("search", "default", "Beef", 5)?;
        dbg!(result);
        # Ok(())
        # }
        ```
        "#]
        use SuggestCommand for fn suggest_with_limit<'a>(
            collection: &'a str,
            bucket: &'a str,
            word: &'a str,
            limit: usize => Some(limit),
        ) where mode is ChannelMode::Search;
    }

    #[cfg(feature = "control")]
    init_commands! {
        #[doc=r#"
        Consolidate indexed search data instead of waiting for the next automated
        consolidation tick.

        Note: This method requires enabling the `control` feature and start 
        connection in Control mode.

        ```rust,no_run
        # use sonic_channel::*;
        # fn main() -> result::Result<()> {
        let control_channel = SonicChannel::connect_with_start(
            ChannelMode::Control,
            "localhost:1491",
            "SecretPassword",
        )?;

        let result = control_channel.consolidate()?;
        assert_eq!(result, true);
        # Ok(())
        # }
        ```
        "#]
        use TriggerCommand for fn consolidate()
            where mode is ChannelMode::Control;

        #[doc=r#"
        Backup KV + FST to <path>/<BACKUP_{KV/FST}_PATH>
        See [sonic backend source code](https://github.com/valeriansaliou/sonic/blob/master/src/channel/command.rs#L808)
        for more information.

        Note: This method requires enabling the `control` feature and start 
        connection in Control mode.

        ```rust,no_run
        # use sonic_channel::*;
        # fn main() -> result::Result<()> {
        let control_channel = SonicChannel::connect_with_start(
            ChannelMode::Control,
            "localhost:1491",
            "SecretPassword",
        )?;

        let result = control_channel.backup("2020-08-07T23-48")?;
        assert_eq!(result, true);
        # Ok(())
        # }
        ```
        "#]
        use TriggerCommand for fn backup<'a>(
            // It's not action, but my macro cannot support alias for custom argument.
            // TODO: Add alias to macro and rename argument of this function.
            action: &'a str => TriggerAction::Backup(action),
        ) where mode is ChannelMode::Control;

        #[doc=r#"
        Restore KV + FST from <path> if you already have backup with the same name.

        Note: This method requires enabling the `control` feature and start 
        connection in Control mode.

        ```rust,no_run
        # use sonic_channel::*;
        # fn main() -> result::Result<()> {
        let control_channel = SonicChannel::connect_with_start(
            ChannelMode::Control,
            "localhost:1491",
            "SecretPassword",
        )?;

        let result = control_channel.restore("2020-08-07T23-48")?;
        assert_eq!(result, true);
        # Ok(())
        # }
        ```
        "#]
        use TriggerCommand for fn restore<'a>(
            // It's not action, but my macro cannot support alias for custom argument.
            // TODO: Add alias to macro and rename argument of this function.
            action: &'a str => TriggerAction::Restore(action),
        ) where mode is ChannelMode::Control;
    }
}
