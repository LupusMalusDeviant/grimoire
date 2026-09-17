using System;
using System.IO;
using System.Text.RegularExpressions;
using Grimoire.Formats.Debug;
using Grimoire.Tools.Tests;
using Xunit;

namespace Grimoire.Formats.Tests;

/// <summary>
/// The build identity baked into the tools (contract §8.1, §13).
/// </summary>
public sealed class EngineBuildTests
{
    [Fact]
    public void TheEngineVersionIsTheWorkspaceVersionOfCargoToml()
    {
        var cargo = File.ReadAllText(RustFixtures.Path("Cargo.toml"));
        var version = Regex.Match(cargo, "(?m)^version = \"([^\"]+)\"").Groups[1].Value;
        Assert.NotEmpty(version);
        Assert.Equal(version, EngineBuild.EngineVersion);
    }

    [Fact]
    public void TheBuildHashFollowsGrimoireBuildHash()
    {
        var environment = Environment.GetEnvironmentVariable("GRIMOIRE_BUILD_HASH");
        if (string.IsNullOrEmpty(environment))
        {
            Assert.Equal(EngineBuild.UnknownBuildHash, EngineBuild.BuildHash);
        }
        else
        {
            Assert.Equal(environment, EngineBuild.BuildHash);
        }
    }

    [Fact]
    public void TheDefaultToolIdentityIsThisBuild()
    {
        var identity = new ToolIdentity(new byte[32]);
        Assert.Equal(EngineBuild.EngineVersion, identity.EngineVersion);
        Assert.Equal(EngineBuild.BuildHash, identity.BuildHash);
        Assert.Throws<ArgumentException>(() => new ToolIdentity(new byte[31]));
    }
}
