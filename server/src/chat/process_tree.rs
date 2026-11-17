use std::io;
use tokio::process::{Child, Command};

#[cfg(unix)]
pub(super) struct ProcessTree(Option<i32>);

#[cfg(unix)]
impl ProcessTree {
    pub(super) fn spawn(command: &mut Command) -> io::Result<(Child, Self)> {
        let child = command.process_group(0).spawn()?;
        Ok((child, Self(child.id().map(|pid| pid as i32))))
    }

    pub(super) fn reaped(&mut self) {
        self.0 = None;
    }
}

#[cfg(unix)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        if let Some(group_id) = self.0 {
            // A negative PID targets the process group created for this invocation.
            unsafe { libc::kill(-group_id, libc::SIGKILL) };
        }
    }
}

#[cfg(windows)]
pub(super) use windows::ProcessTree;

#[cfg(windows)]
mod windows {
    use super::{Child, Command, io};
    use std::mem::size_of;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::ptr::null;
    use windows_sys::Win32::{
        Foundation::{HANDLE, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            },
            Threading::{CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
        },
    };

    pub(in crate::chat) struct ProcessTree {
        _job: OwnedHandle,
    }

    impl ProcessTree {
        pub(in crate::chat) fn spawn(command: &mut Command) -> io::Result<(Child, Self)> {
            let job = owned_handle(unsafe { CreateJobObjectW(null(), null()) })?;
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            check(unsafe {
                SetInformationJobObject(
                    job.as_raw_handle(),
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            })?;

            // Start suspended so no descendant can start before the job is assigned.
            let child = command.creation_flags(CREATE_SUSPENDED).spawn()?;
            let process = child
                .raw_handle()
                .expect("new child must have a process handle");
            check(unsafe { AssignProcessToJobObject(job.as_raw_handle(), process) })?;
            resume_primary_thread(child.id().expect("new child must have a PID"))?;
            Ok((child, Self { _job: job }))
        }

        pub(in crate::chat) fn reaped(&mut self) {}
    }

    fn resume_primary_thread(pid: u32) -> io::Result<()> {
        // std::process closes the primary thread handle. In a newly created suspended
        // process the primary thread is the only thread running its executable.
        let snapshot = owned_handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) })?;
        let handle = snapshot.as_raw_handle();
        let mut entry = THREADENTRY32 {
            dwSize: size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        check(unsafe { Thread32First(handle, &mut entry) })?;
        loop {
            if entry.th32OwnerProcessID == pid {
                let thread = owned_handle(unsafe {
                    OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID)
                })?;
                let count = unsafe { ResumeThread(thread.as_raw_handle()) };
                return if count == u32::MAX {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                };
            }
            entry.dwSize = size_of::<THREADENTRY32>() as u32;
            check(unsafe { Thread32Next(handle, &mut entry) })?;
        }
    }

    fn owned_handle(handle: HANDLE) -> io::Result<OwnedHandle> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }

    fn check(result: i32) -> io::Result<()> {
        if result == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

#[cfg(not(any(unix, windows)))]
pub(super) struct ProcessTree;

#[cfg(not(any(unix, windows)))]
impl ProcessTree {
    pub(super) fn spawn(command: &mut Command) -> io::Result<(Child, Self)> {
        Ok((command.spawn()?, Self))
    }

    pub(super) fn reaped(&mut self) {}
}
